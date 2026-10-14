// Copyright 2026 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![cfg(target_os = "linux")]

use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::ffi::CString;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_worker::namespace_utils;
use nativelink_worker::persistent_worker::{LiveWorker, Namespacing, WireFormat, WorkRequest};
use pretty_assertions::assert_eq;
use utils::network_test_utils::{
    host_interfaces_beyond_loopback, interface_names, only_loopback, skip,
};

mod utils {
    pub(crate) mod network_test_utils;
}

/// Returns a path under `dir` that is unique for this test run.
fn unique_path(dir: &Path, prefix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    #[allow(clippy::cast_possible_truncation)]
    let nanos: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("{prefix}_{}_{nanos}_{count}", std::process::id()))
}

/// Builds `sh -c script` running in a fresh mount namespace for
/// `action_path` under `root_path`, with a private `/tmp` when
/// `isolate_tmp` is set.
/// Like the worker: with `isolate_tmp` the action's own `tmp` directory is
/// created first and bound over `/tmp` in the child.
fn namespaced_sh(script: &str, isolate_tmp: bool, root_path: &Path, action_path: &Path) -> Command {
    let root_dir_c = CString::new(root_path.to_str().unwrap()).unwrap();
    let action_dir_c = CString::new(action_path.to_str().unwrap()).unwrap();
    let tmp_dir_c = isolate_tmp.then(|| {
        let tmp_path = action_path.join("tmp");
        std::fs::create_dir_all(&tmp_path).unwrap();
        CString::new(tmp_path.to_str().unwrap()).unwrap()
    });
    let mut command = Command::new("sh");
    command.args(["-c", script]);
    // SAFETY: configure_namespace is async-signal-safe and intended for pre_exec.
    unsafe {
        command.pre_exec(move || {
            namespace_utils::configure_namespace(
                true,
                false,
                tmp_dir_c.as_deref(),
                &root_dir_c,
                &action_dir_c,
            )
        });
    }
    command
}

#[nativelink_test]
async fn test_namespaces_supported() -> Result<(), Error> {
    // This test is a smoke test to ensure that the namespace detection logic
    // runs without crashing. The result of this function is dependent on the
    // environment it is run in, so we don't assert the result.
    let _supported = namespace_utils::namespaces_supported(false, false, false);
    // Isolating /tmp is only possible inside a mount namespace, regardless of
    // what the host supports.
    assert!(!namespace_utils::namespaces_supported(false, true, false));
    Ok(())
}

#[nativelink_test]
async fn test_configure_namespace_isolate_tmp_hides_host_tmp() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported(true, true, false) {
        return Ok(());
    }

    let root_path = unique_path(&std::env::temp_dir(), "nativelink_test_root");
    let action_path = root_path.join("action");
    std::fs::create_dir_all(&action_path).err_tip(|| "Failed to create action dir")?;

    let host_tmp_file = unique_path(Path::new("/tmp"), "nativelink_test_host");
    std::fs::write(&host_tmp_file, "host").err_tip(|| "Failed to write host /tmp file")?;
    let scratch_file = unique_path(Path::new("/tmp"), "nativelink_test_scratch");

    // The host's /tmp contents must be invisible and the private /tmp must be
    // writable.
    let output = namespaced_sh(
        &format!(
            "test ! -e {host} && echo scratch > {scratch} && test -f {scratch}",
            host = host_tmp_file.display(),
            scratch = scratch_file.display(),
        ),
        true,
        &root_path,
        &action_path,
    )
    .output()?;
    std::fs::remove_file(&host_tmp_file).err_tip(|| "Failed to remove host /tmp file")?;
    assert_eq!(
        Some(0),
        output.status.code(),
        "Host /tmp was visible or the private /tmp was not writable: {output:?}",
    );
    assert!(
        !scratch_file.exists(),
        "Write to the private /tmp leaked to the host at {}",
        scratch_file.display()
    );
    // It landed in the action's own tmp directory, which goes with the action.
    let landed = action_path
        .join("tmp")
        .join(scratch_file.file_name().unwrap());
    assert_eq!(
        "scratch\n",
        std::fs::read_to_string(&landed).err_tip(|| "Failed to read the private /tmp file")?,
        "The private /tmp is not the action's tmp directory"
    );

    Ok(())
}

#[nativelink_test]
async fn test_configure_namespace_isolate_tmp_keeps_action_directory_under_tmp() -> Result<(), Error>
{
    if !namespace_utils::namespaces_supported(true, true, false) {
        return Ok(());
    }

    // Mirror a worker whose work_directory lives under /tmp, which the private
    // tmpfs would otherwise hide.
    let root_path = unique_path(Path::new("/tmp"), "nativelink_test_root");
    let action_path = root_path.join("action");
    let work_path = action_path.join("work");
    let sibling_path = root_path.join("sibling");
    let secret_file = sibling_path.join("secret.txt");
    std::fs::create_dir_all(&work_path).err_tip(|| "Failed to create work dir")?;
    std::fs::create_dir_all(&sibling_path).err_tip(|| "Failed to create sibling dir")?;
    std::fs::write(work_path.join("input.txt"), "input").err_tip(|| "Failed to write input")?;
    std::fs::write(&secret_file, "top secret").err_tip(|| "Failed to write secret file")?;
    let output_file = work_path.join("output.txt");

    // Like the worker, start the command inside the action's work directory.
    // Inputs must be reachable both relatively and by absolute path, siblings
    // must stay masked, and outputs must land in the real work directory.
    let mut command = namespaced_sh(
        &format!(
            "test \"$(pwd)\" = {work} && test -f input.txt && test -f {work}/input.txt && test ! -e {secret} && echo done > output.txt",
            work = work_path.display(),
            secret = secret_file.display(),
        ),
        true,
        &root_path,
        &action_path,
    );
    command.current_dir(&work_path);
    let output = command.output()?;
    assert_eq!(
        Some(0),
        output.status.code(),
        "Action directory under /tmp was not usable: {output:?}",
    );
    assert_eq!(
        "done\n",
        std::fs::read_to_string(&output_file).err_tip(|| "Failed to read output")?,
        "Output written in the work directory did not reach the host"
    );

    Ok(())
}

#[nativelink_test]
async fn test_configure_namespace_isolate_tmp_concurrent_actions_do_not_collide()
-> Result<(), Error> {
    if !namespace_utils::namespaces_supported(true, true, false) {
        return Ok(());
    }

    let root_path = unique_path(&std::env::temp_dir(), "nativelink_test_root");
    let action_path_a = root_path.join("action_a");
    let action_path_b = root_path.join("action_b");
    std::fs::create_dir_all(&action_path_a).err_tip(|| "Failed to create action_a dir")?;
    std::fs::create_dir_all(&action_path_b).err_tip(|| "Failed to create action_b dir")?;

    // Both actions use the same predictable path, like a tool that keys its
    // scratch file on a pid and assumes it owns /tmp.
    let shared_file = unique_path(Path::new("/tmp"), "nativelink_test_shared");
    let script = |owner: &str, delay: &str| {
        format!(
            "sleep {delay} && echo {owner} > {shared} && sleep 0.5 && cat {shared}",
            shared = shared_file.display(),
        )
    };
    let mut command_a = namespaced_sh(&script("A", "0"), true, &root_path, &action_path_a);
    command_a.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut command_b = namespaced_sh(&script("B", "0.2"), true, &root_path, &action_path_b);
    command_b.stdout(Stdio::piped()).stderr(Stdio::piped());

    let child_a = command_a.spawn()?;
    let child_b = command_b.spawn()?;
    let output_a = child_a.wait_with_output()?;
    let output_b = child_b.wait_with_output()?;
    assert_eq!(
        Some(0),
        output_a.status.code(),
        "Action A failed: {output_a:?}"
    );
    assert_eq!(
        Some(0),
        output_b.status.code(),
        "Action B failed: {output_b:?}"
    );
    assert_eq!(
        "A",
        String::from_utf8_lossy(&output_a.stdout).trim(),
        "Action A saw action B's write to the shared /tmp path"
    );
    assert_eq!(
        "B",
        String::from_utf8_lossy(&output_b.stdout).trim(),
        "Action B saw action A's write to the shared /tmp path"
    );
    assert!(
        !shared_file.exists(),
        "Shared /tmp path leaked to the host at {}",
        shared_file.display()
    );

    Ok(())
}

#[nativelink_test]
async fn test_configure_namespace_mount_without_isolate_tmp_keeps_host_tmp() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported(true, false, false) {
        return Ok(());
    }

    let root_path = unique_path(&std::env::temp_dir(), "nativelink_test_root");
    let action_path = root_path.join("action");
    std::fs::create_dir_all(&action_path).err_tip(|| "Failed to create action dir")?;

    let host_tmp_file = unique_path(Path::new("/tmp"), "nativelink_test_host");
    std::fs::write(&host_tmp_file, "host").err_tip(|| "Failed to write host /tmp file")?;

    // Opting out must leave the host's /tmp visible.
    let output = namespaced_sh(
        &format!("test -f {}", host_tmp_file.display()),
        false,
        &root_path,
        &action_path,
    )
    .output()?;
    std::fs::remove_file(&host_tmp_file).err_tip(|| "Failed to remove host /tmp file")?;
    assert_eq!(
        Some(0),
        output.status.code(),
        "Host /tmp should stay visible when isolate_tmp is off: {output:?}",
    );

    Ok(())
}

#[nativelink_test]
async fn test_configure_namespace() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported(false, false, false) {
        return Ok(());
    }

    let mut command = Command::new("sh");
    command.args(["-c", "echo \"uid=$(id -u) pid=$$\"; exit 4"]);

    let root_dir = CString::new("/tmp").unwrap();
    let action_dir = CString::new("/tmp/action").unwrap();

    // SAFETY: `configure_namespace` is designed to be used with `pre_exec`.
    // It is async-signal-safe and will fork, configure the namespace in the
    // child, and the original child process will continue to execute the command.
    unsafe {
        command.pre_exec(move || {
            namespace_utils::configure_namespace(false, false, None, &root_dir, &action_dir)
        });
    }

    let output = command.output()?;
    assert_eq!(
        Some(4),
        output.status.code(),
        "Command failed to execute: {output:?}",
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // SAFETY: `geteuid` is always safe on POSIX systems.
    let expected_uid = unsafe { libc::geteuid() };
    let expected_output = format!("uid={expected_uid} pid=1\n");
    assert_eq!(stdout.trim(), expected_output.trim());

    Ok(())
}

#[nativelink_test]
async fn test_configure_namespace_mount_isolation() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported(true, false, false) {
        return Ok(());
    }

    // Create a temporary root and action directory.
    #[allow(clippy::cast_possible_truncation)]
    let rand_num: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let root_path = std::env::temp_dir().join(format!("nativelink_test_root_{rand_num}"));
    let action1_path = root_path.join("action1");
    let action2_path = root_path.join("action2");
    let secret_file = action2_path.join("secret.txt");

    std::fs::create_dir_all(&action1_path).err_tip(|| "Failed to create action1 dir")?;
    std::fs::create_dir_all(&action2_path).err_tip(|| "Failed to create action2 dir")?;
    std::fs::write(&secret_file, "top secret").err_tip(|| "Failed to write secret file")?;

    let root_dir_c = CString::new(root_path.to_str().unwrap()).unwrap();
    let action1_dir_c = CString::new(action1_path.to_str().unwrap()).unwrap();

    let mut command = Command::new("sh");
    // Try to read the secret file from the sibling directory.
    // Since it's masked by tmpfs, it should not exist.
    // We also check if /bin/sh still works by the fact that this command runs.
    command.args([
        "-c",
        &format!(
            "if [ -f {} ]; then exit 1; else exit 0; fi",
            secret_file.display()
        ),
    ]);

    unsafe {
        command.pre_exec(move || {
            namespace_utils::configure_namespace(true, false, None, &root_dir_c, &action1_dir_c)
        });
    }

    let output = command.output()?;
    assert_eq!(
        Some(0),
        output.status.code(),
        "Sibling directory was not masked or command failed: {output:?}",
    );

    // Verify the action directory itself is still visible and has access to root.
    let mut command_access = Command::new("sh");
    command_access.args(["-c", "ls /bin/sh && exit 0"]);
    let root_dir_c = CString::new(root_path.to_str().unwrap()).unwrap();
    let action1_dir_c = CString::new(action1_path.to_str().unwrap()).unwrap();
    unsafe {
        command_access.pre_exec(move || {
            namespace_utils::configure_namespace(true, false, None, &root_dir_c, &action1_dir_c)
        });
    }
    let output_access = command_access.output()?;
    assert_eq!(
        Some(0),
        output_access.status.code(),
        "Lost access to root filesystem: {output_access:?}",
    );

    Ok(())
}

/// A SIGTERM to the action's process group reaches the action inside its
/// namespaces as a SIGTERM, so a trap runs and the action can write its
/// cleanup before the SIGKILL that follows the grace.
#[nativelink_test]
async fn test_namespaced_action_gets_sigterm_not_sigkill() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported(false, false, false) {
        return Ok(());
    }
    let marker = std::env::temp_dir().join(format!(
        "nativelink_test_term.{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let mut command = tokio::process::Command::new("sh");
    command.args([
        "-c",
        &format!(
            "trap 'echo bye > {marker}; exit 0' TERM; sleep 30 & wait",
            marker = marker.display()
        ),
    ]);
    // As the worker does: the action leads its own process group, and the
    // grace is a SIGTERM to that group.
    command.process_group(0);
    let root_dir = CString::new("/tmp").unwrap();
    let action_dir = CString::new("/tmp/action").unwrap();
    // SAFETY: configure_namespace is async-signal-safe and intended for pre_exec.
    unsafe {
        command.pre_exec(move || {
            namespace_utils::configure_namespace(false, false, None, &root_dir, &action_dir)
        });
    }
    let child = command.spawn()?;
    let pgid = i32::try_from(child.id().unwrap()).unwrap();
    let mut namespaced_child = namespace_utils::MaybeNamespacedChild::new(true, child);
    // Let the shell install its trap and fork the sleep.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // SAFETY: pgid is the group we just created.
    assert_eq!(unsafe { libc::killpg(pgid, libc::SIGTERM) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(5), namespaced_child.wait())
        .await
        .expect("the action should exit on SIGTERM within the grace")?;
    assert_eq!(
        "bye\n",
        std::fs::read_to_string(&marker).err_tip(|| "The trap did not run")?,
        "the action did not get to run its TERM trap"
    );
    assert_eq!(
        status.code(),
        Some(0),
        "the trap's exit code should come through: {status:?}"
    );
    drop(std::fs::remove_file(&marker));
    Ok(())
}

#[nativelink_test]
async fn test_maybe_namespaced_child_kill_reaps_orphans() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported(false, false, false) {
        return Ok(());
    }

    // Use a unique sleep value to identify our processes in the global process list.
    let unique_id = "987654321";
    let mut command = tokio::process::Command::new("sh");
    // We spawn background processes and then wait in the foreground.
    // This ensures the stub has children to reap if the main process is killed.
    command.args([
        "-c",
        &format!("sleep {unique_id} & sleep {unique_id} & wait"),
    ]);

    let root_dir = CString::new("/tmp").unwrap();
    let action_dir = CString::new("/tmp/action").unwrap();

    // SAFETY: configure_namespace is async-signal-safe and intended for pre_exec.
    unsafe {
        command.pre_exec(move || {
            namespace_utils::configure_namespace(false, false, None, &root_dir, &action_dir)
        });
    }

    let child = command.spawn()?;
    let mut namespaced_child = namespace_utils::MaybeNamespacedChild::new(true, child);

    // Give the shell time to fork the background sleep processes.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Kill the stub: SIGKILL to the namespace's init takes 'sh' and its
    // children with it.
    namespaced_child.kill().await?;

    // Ensure the stub process has exited.
    let status = namespaced_child.wait().await?;
    // The stub translates the SIGKILL (9) of the shell into an exit code of 9.
    assert_eq!(status.signal(), Some(9));

    // Verify that no processes with our unique_id exist in the system.
    let mut read_dir = tokio::fs::read_dir("/proc").await?;
    while let Some(entry) = read_dir.next_entry().await? {
        if let Some(s) = entry.file_name().to_str()
            && s.chars().all(|c| c.is_ascii_digit())
        {
            let cmdline = tokio::fs::read_to_string(entry.path().join("cmdline"))
                .await
                .unwrap_or_default();
            if cmdline.contains(unique_id) {
                return Err(nativelink_error::make_err!(
                    nativelink_error::Code::Internal,
                    "Process leaked: PID {} with cmdline '{}'",
                    s,
                    cmdline
                ));
            }
        }
    }

    Ok(())
}

#[nativelink_test]
async fn test_maybe_namespaced_child_non_namespaced_kill() -> Result<(), Error> {
    // This test does not require namespaces to be supported.

    let mut command = tokio::process::Command::new("sleep");
    command.arg("10"); // Sleep for 10 seconds, should be killed before then.

    let child = command.spawn()?;
    let mut non_namespaced_child = namespace_utils::MaybeNamespacedChild::new(false, child);

    // Give the process a moment to start.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Kill the process. This should send SIGKILL directly.
    non_namespaced_child.kill().await?;

    // Ensure the process has exited.
    let status = non_namespaced_child.wait().await?;

    assert_eq!(status.signal(), Some(libc::SIGKILL));

    Ok(())
}

#[nativelink_test]
async fn test_maybe_namespaced_child_namespaced_natural_exit() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported(false, false, false) {
        return Ok(());
    }

    let expected_exit_code: i32 = 5;
    let mut command = tokio::process::Command::new("sh");
    command.args(["-c", &format!("exit {expected_exit_code}")]);

    let root_dir = CString::new("/tmp").unwrap();
    let action_dir = CString::new("/tmp/action").unwrap();

    // SAFETY: configure_namespace is async-signal-safe and intended for pre_exec.
    unsafe {
        command.pre_exec(move || {
            namespace_utils::configure_namespace(false, false, None, &root_dir, &action_dir)
        });
    }

    let child = command.spawn()?;
    let mut namespaced_child = namespace_utils::MaybeNamespacedChild::new(true, child);

    // Wait for the child to exit naturally.
    let status = namespaced_child.wait().await?;

    // The stub should reap the child and return its exit code.
    assert_eq!(status.code(), Some(expected_exit_code));

    Ok(())
}

#[nativelink_test]
async fn test_maybe_namespaced_child_try_wait() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported(false, false, false) {
        return Ok(());
    }

    // Test with a running process
    let mut command_running = tokio::process::Command::new("sleep");
    command_running.arg("10");
    let root_dir = CString::new("/tmp").unwrap();
    let action_dir = CString::new("/tmp/action").unwrap();
    unsafe {
        command_running.pre_exec(move || {
            namespace_utils::configure_namespace(false, false, None, &root_dir, &action_dir)
        });
    }
    let child_running = command_running.spawn()?;
    let mut namespaced_child_running =
        namespace_utils::MaybeNamespacedChild::new(true, child_running);

    // Give it a moment to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    // try_wait should return None as it's still running
    assert!(namespaced_child_running.try_wait()?.is_none());

    // Kill the process to clean up
    namespaced_child_running.kill().await?;
    namespaced_child_running.wait().await?; // Wait for it to actually exit

    // Test with an exited process
    let root_dir = CString::new("/tmp").unwrap();
    let action_dir = CString::new("/tmp/action").unwrap();
    let expected_exit_code: i32 = 7;
    let mut command_exited = tokio::process::Command::new("sh");
    command_exited.args(["-c", &format!("exit {expected_exit_code}")]);
    unsafe {
        command_exited.pre_exec(move || {
            namespace_utils::configure_namespace(false, false, None, &root_dir, &action_dir)
        });
    }
    let child_exited = command_exited.spawn()?;
    let mut namespaced_child_exited =
        namespace_utils::MaybeNamespacedChild::new(true, child_exited);

    // Wait for it to exit
    tokio::time::sleep(Duration::from_millis(50)).await;

    // try_wait should return Some(status)
    let status = namespaced_child_exited.try_wait()?;
    assert_eq!(status.and_then(|s| s.code()), Some(expected_exit_code));

    Ok(())
}

/// An address of this host outside loopback, with a listener behind it, or
/// None when the host has no route out (then there is nothing beyond
/// loopback for an action to reach in the first place). Connecting a UDP
/// socket sends nothing; it only picks the source address a packet to
/// TEST-NET-1 would leave from.
fn outside_listener() -> Option<(std::net::TcpListener, libc::sockaddr_in)> {
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("192.0.2.1:9").ok()?;
    let std::net::SocketAddr::V4(local) = probe.local_addr().ok()? else {
        return None;
    };
    if local.ip().is_loopback() || local.ip().is_unspecified() {
        return None;
    }
    let listener = std::net::TcpListener::bind((*local.ip(), 0)).ok()?;
    let std::net::SocketAddr::V4(listening) = listener.local_addr().ok()? else {
        return None;
    };
    Some((listener, sockaddr_in(listening)))
}

fn sockaddr_in(addr: std::net::SocketAddrV4) -> libc::sockaddr_in {
    // SAFETY: sockaddr_in is plain old data, for which all zeroes is valid.
    let mut sockaddr: libc::sockaddr_in = unsafe { core::mem::zeroed() };
    sockaddr.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap();
    sockaddr.sin_port = addr.port().to_be();
    sockaddr.sin_addr.s_addr = u32::from(*addr.ip()).to_be();
    sockaddr
}

#[allow(clippy::cast_possible_truncation)]
const SOCKADDR_IN_LEN: libc::socklen_t = size_of::<libc::sockaddr_in>() as libc::socklen_t;

fn last_errno() -> i32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO)
}

/// Connect a TCP socket to `addr` from between fork and exec: raw libc and
/// no allocation. Non-blocking, so a connect that the kernel would start
/// (EINPROGRESS) returns at once instead of waiting on a remote host; only
/// "no route at all" fails synchronously with ENETUNREACH.
fn connect_signal_safe(addr: &libc::sockaddr_in, nonblocking: bool) -> Result<(), i32> {
    let mut kind = libc::SOCK_STREAM | libc::SOCK_CLOEXEC;
    if nonblocking {
        kind |= libc::SOCK_NONBLOCK;
    }
    // SAFETY: socket takes only integers.
    let fd = unsafe { libc::socket(libc::AF_INET, kind, 0) };
    if fd < 0 {
        return Err(last_errno());
    }
    // SAFETY: addr is a valid sockaddr_in of SOCKADDR_IN_LEN bytes.
    let rc = unsafe { libc::connect(fd, core::ptr::from_ref(addr).cast(), SOCKADDR_IN_LEN) };
    let result = if rc == 0 { Ok(()) } else { Err(last_errno()) };
    // SAFETY: fd is ours.
    unsafe { libc::close(fd) };
    result
}

/// Listen on 127.0.0.1 and connect to it, from between fork and exec.
fn loopback_round_trip_signal_safe() -> Result<(), i32> {
    // SAFETY: socket takes only integers.
    let listener =
        unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if listener < 0 {
        return Err(last_errno());
    }
    let mut addr = sockaddr_in(std::net::SocketAddrV4::new(
        std::net::Ipv4Addr::LOCALHOST,
        0,
    ));
    let mut len = SOCKADDR_IN_LEN;
    // SAFETY: addr and len are valid for a sockaddr_in on our stack.
    let result =
        if unsafe { libc::bind(listener, core::ptr::from_ref(&addr).cast(), SOCKADDR_IN_LEN) } != 0
            || unsafe { libc::listen(listener, 1) } != 0
            || unsafe {
                libc::getsockname(
                    listener,
                    core::ptr::from_mut(&mut addr).cast(),
                    &raw mut len,
                )
            } != 0
        {
            Err(last_errno())
        } else {
            connect_signal_safe(&addr, false)
        };
    // SAFETY: listener is ours.
    unsafe { libc::close(listener) };
    result
}

/// Checks the network from inside the action's process. Registered as a
/// second `pre_exec` hook, it runs after `configure_namespace` has put the
/// process in its namespaces and forked it into the PID namespace, so it
/// sees exactly the network the action's program would; the stub never
/// returns from the first hook. A failed expectation is returned as an
/// errno, which makes the spawn fail with it.
///
/// Loopback has to work either way. With the network isolated, the host's
/// own outside address and TEST-NET-1 have to be unreachable, with
/// ENETUNREACH and at once; otherwise the host's outside address has to
/// accept the connection. EISCONN stands for "connected where it should
/// not have": no connect here can return it on its own.
fn check_network(
    isolated: bool,
    outside: Option<libc::sockaddr_in>,
    test_net: libc::sockaddr_in,
) -> std::io::Result<()> {
    let fail = std::io::Error::from_raw_os_error;
    loopback_round_trip_signal_safe().map_err(fail)?;
    if isolated {
        for addr in outside.iter().chain(core::iter::once(&test_net)) {
            match connect_signal_safe(addr, true) {
                Err(libc::ENETUNREACH) => {}
                Err(libc::EINPROGRESS) | Ok(()) => return Err(fail(libc::EISCONN)),
                Err(errno) => return Err(fail(errno)),
            }
        }
    } else if let Some(addr) = outside {
        connect_signal_safe(&addr, false).map_err(fail)?;
    }
    Ok(())
}

/// Runs `sh` in fresh namespaces, with or without the network isolated,
/// checks the network from inside it, and returns the interfaces it saw.
fn run_network_probe(
    isolate_network: bool,
    outside: Option<libc::sockaddr_in>,
) -> Result<std::collections::BTreeSet<String>, Error> {
    let test_net = sockaddr_in(std::net::SocketAddrV4::new(
        std::net::Ipv4Addr::new(192, 0, 2, 1),
        9,
    ));
    let root_dir = CString::new("/tmp").unwrap();
    let action_dir = CString::new("/tmp/action").unwrap();
    let mut command = Command::new("sh");
    // Builtins only, so this needs nothing on PATH beyond sh itself.
    command.args([
        "-c",
        "while read -r line; do echo \"$line\"; done < /proc/net/dev",
    ]);
    // SAFETY: Both hooks are async-signal-safe and intended for pre_exec.
    unsafe {
        command.pre_exec(move || {
            namespace_utils::configure_namespace(
                false,
                isolate_network,
                None,
                &root_dir,
                &action_dir,
            )
        });
        command.pre_exec(move || check_network(isolate_network, outside, test_net));
    }
    let output = command.output().map_err(|err| {
        Error::from_std_err(nativelink_error::Code::Internal, &err).append(
            "The network check inside the namespace failed; EISCONN means an address outside loopback was reachable",
        )
    })?;
    assert!(output.status.success(), "{output:?}");
    Ok(interface_names(&String::from_utf8_lossy(&output.stdout)))
}

const NO_NETWORK_NAMESPACES: &str = "network namespaces are unavailable here";

#[nativelink_test]
async fn test_isolate_network_keeps_loopback_and_nothing_else() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported(false, false, true) {
        skip(
            "test_isolate_network_keeps_loopback_and_nothing_else",
            NO_NETWORK_NAMESPACES,
        );
        return Ok(());
    }
    let outside = outside_listener();
    let interfaces = run_network_probe(true, outside.as_ref().map(|(_, addr)| *addr))?;
    assert_eq!(
        interfaces,
        only_loopback(),
        "an isolated action should see only its own loopback interface"
    );
    Ok(())
}

#[nativelink_test]
async fn test_without_isolate_network_the_host_network_stays_reachable() -> Result<(), Error> {
    const TEST: &str = "test_without_isolate_network_the_host_network_stays_reachable";
    if !namespace_utils::namespaces_supported(false, false, false) {
        skip(TEST, "namespaces are unavailable here");
        return Ok(());
    }
    let Some(host_interfaces) = host_interfaces_beyond_loopback(TEST) else {
        return Ok(());
    };
    let outside = outside_listener();
    let interfaces = run_network_probe(false, outside.as_ref().map(|(_, addr)| *addr))?;
    assert_eq!(
        interfaces, host_interfaces,
        "without isolate_network an action shares the worker's network namespace"
    );
    Ok(())
}

#[nativelink_test]
async fn test_namespaces_supported_checks_the_network_namespace() -> Result<(), Error> {
    // Where a network namespace cannot be made the answer is false, which
    // is the startup failure; where it can, the probe brings loopback up in
    // it like an action's namespace does, and must not fail on that.
    if !namespace_utils::namespaces_supported(false, false, false) {
        skip(
            "test_namespaces_supported_checks_the_network_namespace",
            "namespaces are unavailable here",
        );
        return Ok(());
    }
    let with_network = namespace_utils::namespaces_supported(false, false, true);
    let can_unshare_net = run_network_probe(true, None).is_ok();
    assert_eq!(with_network, can_unshare_net);
    Ok(())
}

/// A persistent worker process started with the network isolated sees only
/// loopback, as a one-shot action does. It answers its one request with the
/// interface names in its own `/proc/net/dev`.
#[nativelink_test]
async fn test_isolate_network_applies_to_persistent_worker_processes() -> Result<(), Error> {
    const TEST: &str = "test_isolate_network_applies_to_persistent_worker_processes";
    if !namespace_utils::namespaces_supported(false, false, true) {
        skip(TEST, NO_NETWORK_NAMESPACES);
        return Ok(());
    }
    if host_interfaces_beyond_loopback(TEST).is_none() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let script = dir.path().join("worker.sh");
    std::fs::write(
        &script,
        r#"read -r request
names=
{ read -r header; read -r header; while read -r l; do names="$names ${l%%:*}"; done; } < /proc/net/dev
echo "{\"exitCode\":0,\"output\":\"$names\"}"
"#,
    )?;
    let mut worker = LiveWorker::spawn(
        Path::new("/bin/sh"),
        &[script.display().to_string()],
        WireFormat::Json,
        dir.path(),
        &[(
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_default(),
        )],
        Namespacing::Yes {
            isolate_network: true,
        },
    )?;
    let response = worker.dispatch(&WorkRequest::default()).await?;
    worker.shutdown(Duration::from_secs(1)).await;
    assert_eq!(response.exit_code, 0, "{response:?}");
    assert_eq!(
        response
            .output
            .split_whitespace()
            .map(String::from)
            .collect::<std::collections::BTreeSet<_>>(),
        only_loopback(),
    );
    Ok(())
}
