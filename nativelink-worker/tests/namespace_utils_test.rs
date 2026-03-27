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

use core::time::Duration;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Command;

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_worker::namespace_utils;
use pretty_assertions::assert_eq;

#[nativelink_test]
async fn test_namespaces_supported() -> Result<(), Error> {
    // This test is a smoke test to ensure that the namespace detection logic
    // runs without crashing. The result of this function is dependent on the
    // environment it is run in, so we don't assert the result.
    let _supported = namespace_utils::namespaces_supported();
    Ok(())
}

#[nativelink_test]
async fn test_configure_namespace() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported() {
        return Ok(());
    }

    let mut command = Command::new("sh");
    command.args(["-c", "echo \"uid=$(id -u) pid=$$\"; exit 4"]);

    // SAFETY: `configure_namespace` is designed to be used with `pre_exec`.
    // It is async-signal-safe and will fork, configure the namespace in the
    // child, and the original child process will continue to execute the command.
    unsafe {
        command.pre_exec(namespace_utils::configure_namespace);
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
async fn test_maybe_namespaced_child_kill_reaps_orphans() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported() {
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

    // SAFETY: configure_namespace is async-signal-safe and intended for pre_exec.
    unsafe {
        command.pre_exec(namespace_utils::configure_namespace);
    }

    let child = command.spawn()?;
    let mut namespaced_child = namespace_utils::MaybeNamespacedChild::new(true, child);

    // Give the shell time to fork the background sleep processes.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Kill the stub. This sends SIGTERM to the stub, which sends SIGKILL to 'sh'.
    namespaced_child.kill().await?;

    // Ensure the stub process has exited.
    let status = namespaced_child.wait().await?;
    // The stub translates the SIGKILL (9) of the shell into an exit code of 9.
    assert_eq!(status.signal(), Some(9));

    // Verify that no processes with our unique_id exist in the system.
    let mut read_dir = tokio::fs::read_dir("/proc").await?;
    while let Some(entry) = read_dir.next_entry().await? {
        if let Some(s) = entry.file_name().to_str() {
            if s.chars().all(|c| c.is_ascii_digit()) {
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
    if !namespace_utils::namespaces_supported() {
        return Ok(());
    }

    const EXPECTED_EXIT_CODE: i32 = 5;
    let mut command = tokio::process::Command::new("sh");
    command.args(["-c", &format!("exit {EXPECTED_EXIT_CODE}")]);

    // SAFETY: configure_namespace is async-signal-safe and intended for pre_exec.
    unsafe {
        command.pre_exec(namespace_utils::configure_namespace);
    }

    let child = command.spawn()?;
    let mut namespaced_child = namespace_utils::MaybeNamespacedChild::new(true, child);

    // Wait for the child to exit naturally.
    let status = namespaced_child.wait().await?;

    // The stub should reap the child and return its exit code.
    assert_eq!(status.code(), Some(EXPECTED_EXIT_CODE));

    Ok(())
}

#[nativelink_test]
async fn test_maybe_namespaced_child_try_wait() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported() {
        return Ok(());
    }

    // Test with a running process
    let mut command_running = tokio::process::Command::new("sleep");
    command_running.arg("10");
    unsafe {
        command_running.pre_exec(namespace_utils::configure_namespace);
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
    const EXPECTED_EXIT_CODE: i32 = 7;
    let mut command_exited = tokio::process::Command::new("sh");
    command_exited.args(["-c", &format!("exit {EXPECTED_EXIT_CODE}")]);
    unsafe {
        command_exited.pre_exec(namespace_utils::configure_namespace);
    }
    let child_exited = command_exited.spawn()?;
    let mut namespaced_child_exited =
        namespace_utils::MaybeNamespacedChild::new(true, child_exited);

    // Wait for it to exit
    tokio::time::sleep(Duration::from_millis(50)).await;

    // try_wait should return Some(status)
    let status = namespaced_child_exited.try_wait()?;
    assert_eq!(status.and_then(|s| s.code()), Some(EXPECTED_EXIT_CODE));

    Ok(())
}
