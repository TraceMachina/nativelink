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

use std::io::Error;

use tracing::error;

/// A wrapper around a Child that may be the stub of a PID namespace. A
/// SIGTERM to the stub is passed on to the action; a SIGKILL to the stub
/// ends the whole namespace, which is the hard kill.
#[derive(Debug)]
pub struct MaybeNamespacedChild {
    namespaced: bool,
    child: tokio::process::Child,
}

/// SIGKILL to every process in the group `pgid` leads. ESRCH means the
/// group is already gone, which is the outcome wanted; anything else, EPERM
/// above all, means a survivor and is reported.
#[cfg(target_os = "linux")]
fn kill_process_group(pgid: u32) {
    let Ok(pgid) = i32::try_from(pgid) else {
        return;
    };
    // SAFETY: killpg only takes integers and has no memory safety
    // considerations; a stale group id is reported as ESRCH, not acted on.
    let rc = unsafe { libc::killpg(pgid, libc::SIGKILL) };
    if rc != 0 {
        let err = Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            error!(
                pgid,
                ?err,
                "Could not kill the action's process group; a descendant may have survived"
            );
        }
    }
}

impl MaybeNamespacedChild {
    pub const fn new(namespaced: bool, child: tokio::process::Child) -> Self {
        Self { namespaced, child }
    }

    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// SIGKILL the child and wait for it. Namespaced, the child is the
    /// stub and the init of the action's PID namespace, so its death takes
    /// the action and everything the action spawned with it; the stub also
    /// set `PR_SET_PDEATHSIG` on the action for the same end. Not
    /// namespaced, the child is its own group leader, so the group gets the
    /// SIGKILL first: a timed-out or cancelled action used to lose only its
    /// direct child while its descendants ran on (issue #225).
    pub async fn kill(&mut self) -> Result<(), Error> {
        #[cfg(target_os = "linux")]
        if !self.namespaced
            && let Some(pgid) = self.child.id()
        {
            kill_process_group(pgid);
        }
        self.child.kill().await
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, Error> {
        self.child.try_wait()
    }

    pub async fn wait(&mut self) -> Result<std::process::ExitStatus, Error> {
        self.child.wait().await
    }
}

fn exit(status: i32) -> ! {
    // SAFETY: It is always safe to _exit.
    unsafe { libc::_exit(status) };
}

#[derive(Clone, Copy)]
enum NamespaceErrorType {
    Unshare = 1,
    WriteSignalSafe,
    Mount,
    Loopback,
}

/// The namespaces every namespaced action is put in, plus the mount and
/// network namespaces when asked for.
const fn unshare_flags(mount: bool, isolate_network: bool) -> libc::c_int {
    let mut flags =
        libc::CLONE_NEWPID | libc::CLONE_NEWUSER | libc::CLONE_NEWIPC | libc::CLONE_NEWUTS;
    if mount {
        flags |= libc::CLONE_NEWNS;
    }
    if isolate_network {
        flags |= libc::CLONE_NEWNET;
    }
    flags
}

/// Ends the probe's child: the errno goes to the parent over `report`,
/// whole, and the exit status carries only the step that failed. An exit
/// status has eight bits, too few to hold every errno beside the step.
fn probe_failed(report: libc::c_int, error_type: NamespaceErrorType, errno: i32) -> ! {
    let errno = errno.to_ne_bytes();
    // SAFETY: write is async-signal-safe and reads only our stack. If it
    // fails the parent logs the step without an errno.
    unsafe { libc::write(report, errno.as_ptr().cast(), errno.len()) };
    exit(error_type as i32);
}

/// Determines whether the namespaces provided by this module are supported
/// on the currently running system by forking a process and trying to enter
/// it into the new namespaces. When `mount` is set the mount namespace is
/// checked as well, and when `isolate_tmp` is also set the check includes
/// bind mounting over `/tmp`. When `isolate_network` is set the check
/// includes a network namespace and bringing its loopback interface up.
pub fn namespaces_supported(mount: bool, isolate_tmp: bool, isolate_network: bool) -> bool {
    if isolate_tmp && !mount {
        error!("Namespaces: isolating /tmp requires a mount namespace");
        return false;
    }
    if isolate_tmp && !std::path::Path::new("/tmp").is_dir() {
        error!("Namespaces: isolating /tmp requires /tmp to exist and be a directory");
        return false;
    }
    // SAFETY: Posix requires that geteuid and getegid are always successful.
    let uid = unsafe { libc::geteuid() };
    // SAFETY: As above.
    let gid = unsafe { libc::getegid() };
    // Read before the fork: once the child has unshared, its ids are the
    // unmapped overflow ids until the maps are written.
    let uid_map = format!("{uid} {uid} 1\n");
    let gid_map = format!("{gid} {gid} 1\n");
    // The child reports the errno of a failed step over this pipe.
    let mut fds = [0; 2];
    // SAFETY: pipe2 writes two descriptors into the array on our stack.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        error!(
            errno = Error::last_os_error().raw_os_error(),
            "Namespaces: could not create the pipe for the namespace check"
        );
        return false;
    }
    let (report_read, report_write) = (OwnedFd(fds[0]), OwnedFd(fds[1]));
    // SAFETY: We ensure that if pid == 0 we only call async-signal-safe functions.
    let pid = unsafe { libc::fork() };
    match pid {
        0 => {
            let report = report_write.0;
            let flags = unshare_flags(mount, isolate_network);
            // SAFETY: Unshare does not have any unsafe effects and modifies no
            // memory, it is also async-signal-safe.
            if unsafe { libc::unshare(flags) } == 0 {
                // The maps `configure_namespace` writes for an action, in
                // its order and with its tolerance. The gid has to be mapped
                // too: an inode created on a filesystem mounted inside the
                // namespace, as the mkdir below does, needs the creator's
                // group to map, or the kernel refuses it with EOVERFLOW.
                if let Err(err) = write_signal_safe(c"/proc/self/setgroups", b"deny")
                    && err != libc::EPERM
                    && err != libc::EACCES
                    && err != libc::ENOENT
                {
                    probe_failed(report, NamespaceErrorType::WriteSignalSafe, err);
                }
                match write_signal_safe(c"/proc/self/uid_map", uid_map.as_bytes()) {
                    Ok(()) => {
                        if let Err(err) =
                            write_signal_safe(c"/proc/self/gid_map", gid_map.as_bytes())
                            && err != libc::EPERM
                            && err != libc::EACCES
                        {
                            probe_failed(report, NamespaceErrorType::WriteSignalSafe, err);
                        }
                        // As `configure_namespace` does it for an action, so
                        // a host that lets a network namespace be created
                        // but not configured fails here, at startup.
                        if isolate_network && let Err(err) = bring_up_loopback() {
                            let errno = err.raw_os_error().unwrap_or(libc::EIO);
                            probe_failed(report, NamespaceErrorType::Loopback, errno);
                        }
                        if !mount {
                            exit(0);
                        }
                        // SAFETY: Mount uses no memory and is async-signal-safe.
                        if unsafe {
                            libc::mount(
                                core::ptr::null(),
                                c"/".as_ptr(),
                                core::ptr::null(),
                                libc::MS_REC | libc::MS_PRIVATE,
                                core::ptr::null(),
                            )
                        } != 0
                        {
                            // SAFETY: We just called a libc function that failed (-1).
                            let errno = unsafe { *libc::__errno_location() };
                            probe_failed(report, NamespaceErrorType::Mount, errno);
                        }
                        // The same sequence `perform_remount` runs for an
                        // action, so a policy that allows a bind to self but
                        // refuses a sized tmpfs, or a mkdir under it, fails
                        // here and not in every action's pre_exec. All of it
                        // is inside this child's private mount namespace.
                        if isolate_tmp
                            && let Err(err) = bind_mount(c"/tmp", c"/tmp")
                                .and_then(|()| mount_tmpfs(c"/tmp", Some(ROOT_MASK_TMPFS_OPTIONS)))
                                .and_then(|()| {
                                    mkdir_p_signal_safe(c"/tmp/nativelink-namespace-probe/action")
                                })
                        {
                            let errno = err.raw_os_error().unwrap_or(libc::EIO);
                            probe_failed(report, NamespaceErrorType::Mount, errno);
                        }
                        exit(0);
                    }
                    Err(uid_map_err) => {
                        probe_failed(report, NamespaceErrorType::WriteSignalSafe, uid_map_err);
                    }
                }
            }
            // SAFETY: We just called a libc function that failed (-1).
            let errno = unsafe { *libc::__errno_location() };
            probe_failed(report, NamespaceErrorType::Unshare, errno);
        }
        pid if pid > 0 => {
            // Only the child writes: with our copy closed, the read below
            // ends when the child does.
            drop(report_write);
            let mut status = 0;
            // SAFETY: The pid is valid and created by us and the status is our own stack.
            while unsafe { libc::waitpid(pid, &raw mut status, 0) } == -1 {
                // SAFETY: We just called a libc function that failed (-1).
                let errno = unsafe { *libc::__errno_location() };
                if errno != libc::EINTR {
                    error!(errno = errno, "Namespaces: Failure in waitpid");
                    return false;
                }
            }
            let mut errno = [0_u8; 4];
            // SAFETY: read writes at most errno.len() bytes into our stack.
            let read = unsafe { libc::read(report_read.0, errno.as_mut_ptr().cast(), errno.len()) };
            let errno = (read == 4).then(|| i32::from_ne_bytes(errno));
            if libc::WIFEXITED(status) {
                match libc::WEXITSTATUS(status) {
                    0 => {
                        return true;
                    }
                    s if s == NamespaceErrorType::Unshare as i32 => {
                        error!(errno, "Namespaces: Error during unshare");
                        if errno == Some(libc::EPERM) {
                            error!(
                                "If the worker is inside Docker, namespaces don't work unless it's a privileged container"
                            );
                        }
                    }
                    s if s == NamespaceErrorType::WriteSignalSafe as i32 => {
                        error!(
                            errno,
                            "Namespaces: Error while writing the id maps under /proc/self"
                        );
                    }
                    s if s == NamespaceErrorType::Mount as i32 => {
                        error!(errno, "Failure to mount during namespace checking");
                    }
                    s if s == NamespaceErrorType::Loopback as i32 => {
                        error!(
                            errno,
                            "Namespaces: could not bring up the loopback interface in a new network namespace"
                        );
                    }
                    other => {
                        error!(
                            exit_code = other,
                            "Namespace check failure with unknown exit code"
                        );
                    }
                }
            } else {
                error!(
                    exit_code = status,
                    "Namespaces: waitpid exit with non-exit code"
                );
            }
            false
        }
        _ => false,
    }
}

/// Writes to a file in an async-signal-safe manner, does the write in a
/// single chunk and assumes it will all be consumed, if the whole chunk
/// is not written returns Err(EIO).  This is expected to be used for
/// special files such as /proc which will always accept the whole buffer.
fn write_signal_safe(file_name: &core::ffi::CStr, data: &[u8]) -> Result<(), core::ffi::c_int> {
    // SAFETY: The path is a CStr which is guaranteed to end in a NUL byte
    // and the returned file descriptor is always closed.
    let fd = unsafe { libc::open(file_name.as_ptr().cast(), libc::O_WRONLY) };
    if fd < 0 {
        // SAFETY: We just called a libc function that failed (-1).
        return Err(unsafe { *libc::__errno_location() });
    }
    let fd = OwnedFd(fd);

    // SAFETY: The data is a known length slice and the file descriptor is
    // known to be valid as we just opened it.
    let bytes_written = unsafe { libc::write(fd.0, data.as_ptr().cast(), data.len()) };

    if bytes_written == -1 {
        // SAFETY: We just called a libc function that failed (-1).
        Err(unsafe { *libc::__errno_location() })
    } else if bytes_written as usize != data.len() {
        Err(libc::EIO)
    } else {
        Ok(())
    }
}

/// An async-signal-safe method to close all open file descriptors for the
/// current process.  This function is unsafe as any existing handles to
/// file descriptors will be invalidated.  None may be used after calling
/// this function.
unsafe fn close_all_fds() {
    // SAFETY: It is safe to call close on all file descriptors as this is
    // the purpose of the function.
    if unsafe { libc::syscall(libc::SYS_close_range, 0, libc::INT_MAX, 0) } == 0 {
        return;
    }
    // Since we're <5.9 kernel, we need to get the max FD count.
    let mut rlim = core::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: We just allocated the memory for this and getrlimit is async-signal-safe.
    let max_fd = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, rlim.as_mut_ptr()) } == 0 {
        // SAFETY: We just initialised this in getrlimit above that succeeded.
        let cur = unsafe { rlim.assume_init().rlim_cur };
        if cur == libc::RLIM_INFINITY {
            // Sane fallback for unlimited environments
            0x0001_0000
        } else {
            core::ffi::c_int::try_from(cur).unwrap_or(0x0001_0000)
        }
    } else {
        // Fallback for getrlimit failure.
        4096
    };
    for fd in 0..max_fd {
        // SAFETY: It is safe to close a file descriptor that is not open and
        // we also want to close all, so there's no issue with closing file
        // descriptors that others may have handles to.
        unsafe { libc::close(fd) };
    }
}

/// Write the value n to the given slice as a decimal string.
fn u32_to_bytes(mut n: u32, buf: &mut [u8]) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    buf[..i].reverse();
    i
}

/// Create a line in the buffer of the format "{id} {id} 1\n" in an
/// async-signal-safe manner.
fn create_map_line(id: u32, buffer: &mut [u8; 32]) -> &'_ [u8] {
    let mut pos = 0;
    pos += u32_to_bytes(id, &mut buffer[pos..]);
    buffer[pos] = b' ';
    pos += 1;
    pos += u32_to_bytes(id, &mut buffer[pos..]);
    buffer[pos] = b' ';
    pos += 1;
    buffer[pos] = b'1';
    pos += 1;
    buffer[pos] = b'\n';
    pos += 1;
    &buffer[..pos]
}

/// A simple wrapper around a file descriptor to ensure async-signal-safety
/// rather than the std version which may allocate.
struct OwnedFd(libc::c_int);

impl Drop for OwnedFd {
    fn drop(&mut self) {
        // SAFETY: We own the file descriptor, so we can close it.
        unsafe {
            libc::close(self.0);
        }
    }
}

/// `SIOCGIFFLAGS` and `SIOCSIFFLAGS` from `linux/sockios.h`, the same on
/// every Linux architecture. Spelled here because the type of `ioctl`'s
/// request argument differs between glibc (`c_ulong`, the type of libc's
/// constants) and musl (`c_int`), and a literal fits both without a cast.
const SIOCGIFFLAGS: libc::Ioctl = 0x8913;
const SIOCSIFFLAGS: libc::Ioctl = 0x8914;
/// `IFF_UP` from `linux/if.h`, as the `c_short` that `ifr_flags` is.
const IFF_UP: libc::c_short = 0x1;

/// Bring the loopback interface of the current network namespace up, in an
/// async-signal-safe manner. A new network namespace has a loopback
/// interface and nothing else, and it starts down, so without this even
/// `127.0.0.1` is unreachable. The kernel gives an up loopback interface
/// its `127.0.0.1/8` and `::1` addresses by itself.
fn bring_up_loopback() -> Result<(), Error> {
    // SAFETY: socket is async-signal-safe and takes only integers; the
    // descriptor is closed by the OwnedFd below.
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(Error::last_os_error());
    }
    let fd = OwnedFd(fd);
    // SAFETY: ifreq is plain old data, for which all zeroes is valid.
    let mut request: libc::ifreq = unsafe { core::mem::zeroed() };
    for (dst, src) in request.ifr_name.iter_mut().zip(b"lo") {
        // c_char is i8 or u8 depending on the architecture.
        *dst = libc::c_char::from_ne_bytes([*src]);
    }
    // SAFETY: ioctl is async-signal-safe; the request is a valid ifreq on
    // our stack, NUL-terminated by the zeroing above.
    if unsafe { libc::ioctl(fd.0, SIOCGIFFLAGS, &raw mut request) } != 0 {
        return Err(Error::last_os_error());
    }
    // SAFETY: SIOCGIFFLAGS filled in the flags member of the union.
    unsafe { request.ifr_ifru.ifru_flags |= IFF_UP };
    // SAFETY: As above.
    if unsafe { libc::ioctl(fd.0, SIOCSIFFLAGS, &raw mut request) } != 0 {
        return Err(Error::last_os_error());
    }
    Ok(())
}

/// With a private `/tmp`, the tmpfs that masks the root action directory
/// holds one directory entry and a mount point, so it is bounded to this:
/// anything that writes beside the action directory instead of inside it
/// gets `ENOSPC` rather than the worker's memory. Without `isolate_tmp`
/// the mask is unbounded, as it was before the private `/tmp` existed.
const ROOT_MASK_TMPFS_OPTIONS: &core::ffi::CStr = c"size=1m";

/// Whether `path` is `/tmp` or lies under it.
fn is_under_tmp(path: &core::ffi::CStr) -> bool {
    let bytes = path.to_bytes();
    bytes == b"/tmp" || bytes.starts_with(b"/tmp/")
}

/// Mount a fresh, empty tmpfs over the given directory in an
/// async-signal-safe manner, with the given mount options, or none.
fn mount_tmpfs(target: &core::ffi::CStr, options: Option<&core::ffi::CStr>) -> Result<(), Error> {
    // SAFETY: mount is async-signal-safe. The filesystem type, target and options are valid C-strings.
    if unsafe {
        libc::mount(
            c"tmpfs".as_ptr(),
            target.as_ptr(),
            c"tmpfs".as_ptr(),
            0,
            options.map_or(core::ptr::null(), |options| options.as_ptr().cast()),
        )
    } != 0
    {
        return Err(Error::last_os_error());
    }
    Ok(())
}

/// Bind mount `source` over `target` in an async-signal-safe manner.
fn bind_mount(source: &core::ffi::CStr, target: &core::ffi::CStr) -> Result<(), Error> {
    // SAFETY: mount is async-signal-safe. Both paths are valid C-strings.
    if unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            core::ptr::null(),
            libc::MS_BIND | libc::MS_REC,
            core::ptr::null(),
        )
    } != 0
    {
        return Err(Error::last_os_error());
    }
    Ok(())
}

/// Create a directory and any missing parents in an async-signal-safe
/// manner, like `mkdir -p`. Components that already exist are skipped, so
/// this is a no-op for a path that is already present.
fn mkdir_p_signal_safe(path: &core::ffi::CStr) -> Result<(), Error> {
    let bytes = path.to_bytes();
    // Leave room for the NUL that terminates each prefix.
    let mut buffer = [0u8; libc::PATH_MAX as usize];
    if bytes.len() >= buffer.len() {
        return Err(Error::from_raw_os_error(libc::ENAMETOOLONG));
    }
    buffer[..bytes.len()].copy_from_slice(bytes);
    // Start at 1 so a leading '/' is never treated as an empty component.
    for i in 1..=bytes.len() {
        if i < bytes.len() && buffer[i] != b'/' {
            continue;
        }
        // Temporarily terminate the path here so just this prefix is created.
        buffer[i] = 0;
        // SAFETY: mkdir is async-signal-safe and the buffer is NUL-terminated at i.
        if unsafe { libc::mkdir(buffer.as_ptr().cast(), 0o777) } != 0 {
            let err = Error::last_os_error();
            if err.raw_os_error() != Some(libc::EEXIST) {
                return Err(err);
            }
        }
        if i < bytes.len() {
            buffer[i] = b'/';
        }
    }
    Ok(())
}

fn perform_remount(
    tmp_directory: Option<&core::ffi::CStr>,
    root_action_directory: &core::ffi::CStr,
    action_directory: &core::ffi::CStr,
) -> Result<(), Error> {
    // Make the mount namespace private to avoid changes propagating back to the host.
    // SAFETY: mount is async-signal-safe. We pass a null pointer for the source and valid
    // C-string pointers for the target. The parameters match POSIX requirements.
    if unsafe {
        libc::mount(
            core::ptr::null(),
            c"/".as_ptr(),
            core::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            core::ptr::null(),
        )
    } != 0
    {
        return Err(Error::last_os_error());
    }

    // Bind mount the action directory to itself to "save" its current contents before
    // we mask its parent.
    // SAFETY: mount is async-signal-safe. We pass valid C-string pointers for the paths.
    if unsafe {
        libc::mount(
            action_directory.as_ptr(),
            action_directory.as_ptr(),
            core::ptr::null(),
            libc::MS_BIND | libc::MS_REC,
            core::ptr::null(),
        )
    } != 0
    {
        return Err(Error::last_os_error());
    }

    // Open the directory with O_PATH so we can find it after masking the parent.
    // SAFETY: open is async-signal-safe. The path is a valid C-string.
    let fd = unsafe { libc::open(action_directory.as_ptr(), libc::O_PATH) };
    if fd < 0 {
        return Err(Error::last_os_error());
    }
    let fd = OwnedFd(fd);

    if let Some(tmp_directory) = tmp_directory {
        // Give the action a private /tmp: its own tmp directory, on the
        // worker's disk and removed with the action, bound over /tmp (the
        // bind carries the work volume's mount options, noexec and the
        // like, not the host /tmp's; the config doc says so) so
        // concurrent actions cannot collide on predictable paths there and
        // nothing leaks between actions through it. A directory on disk
        // rather than a tmpfs, because tmpfs pages are memory charged to
        // the action's cgroup: a tool filling a memory-backed /tmp would
        // take the worker down with it, where filling a directory under
        // the scratch volume is bounded by that volume and the disk guard.
        // This has to happen before the root action directory is masked: if
        // that directory lives under /tmp the bind hides it, so its path is
        // recreated inside the bound directory and the action directory
        // itself is restored below from the saved file descriptor. When the
        // root action directory lives elsewhere every component already
        // exists and the mkdir is a no-op.
        bind_mount(tmp_directory, c"/tmp")?;
        mkdir_p_signal_safe(root_action_directory)?;
    }

    // Mask the root action directory with a tmpfs to ensure sibling
    // directories aren't visible. With the root at or under a bound /tmp
    // the bind already hides every sibling, and a mask there would either
    // land on top of the bind (root exactly /tmp: the action's "private
    // /tmp" would be the 1 MiB mask) or leave a mount point sitting in the
    // action's otherwise empty /tmp, so it is skipped.
    let root_under_bound_tmp = tmp_directory.is_some() && is_under_tmp(root_action_directory);
    if !root_under_bound_tmp {
        let options = tmp_directory.is_some().then_some(ROOT_MASK_TMPFS_OPTIONS);
        mount_tmpfs(root_action_directory, options)?;
    }

    // Recreate the specific operation's directory inside the empty tmpfs.
    // SAFETY: mkdir is async-signal-safe and the path is a valid C-string.
    if unsafe { libc::mkdir(action_directory.as_ptr(), 0o777) } != 0 {
        return Err(Error::last_os_error());
    }

    // Bind mount the saved directory back from the file descriptor to the new path.
    let mut proc_path = [0u8; 64];
    let mut pos = 0;
    for &b in b"/proc/self/fd/" {
        proc_path[pos] = b;
        pos += 1;
    }
    pos += u32_to_bytes(fd.0 as u32, &mut proc_path[pos..]);
    proc_path[pos] = 0;

    // SAFETY: mount is async-signal-safe. The target path is a valid C-string and the source
    // path is correctly formatted using /proc/self/fd/.
    if unsafe {
        libc::mount(
            proc_path.as_ptr().cast(),
            action_directory.as_ptr(),
            core::ptr::null(),
            libc::MS_BIND | libc::MS_REC,
            core::ptr::null(),
        )
    } != 0
    {
        return Err(Error::last_os_error());
    }

    Ok(())
}

/// A hook for a `Command::spawn` to create the process in a new namespace.
/// This creates a stub process that the Command points at which forwards
/// SIGKILL to the actual process in the new user, PID, UTS and IPC
/// namespaces.  Pass this function to `CommandBuilder::pre_exec`.
///
/// When `mount` is set the process also gets its own mount namespace in
/// which the root action directory is masked so sibling actions are not
/// visible. When `tmp_directory` is given as well, that directory (the
/// action's own, already created) is bound over `/tmp`, so the action has
/// a private `/tmp` that is removed with it.
///
/// When `isolate_network` is set the process also gets its own network
/// namespace, in which only the loopback interface exists, brought up so
/// the action can still talk to itself over `127.0.0.1` and `::1`.
///
/// This function is async-signal-safe and has no external locks or
/// memory allocations.
pub fn configure_namespace(
    mount: bool,
    isolate_network: bool,
    tmp_directory: Option<&core::ffi::CStr>,
    root_action_directory: &core::ffi::CStr,
    action_directory: &core::ffi::CStr,
) -> std::io::Result<()> {
    // SAFETY: It is always safe to call geteuid on Posix.
    let uid = unsafe { libc::geteuid() };
    // SAFETY: It is always safe to call getegid on Posix.
    let gid = unsafe { libc::getegid() };

    // The new network namespace, like the others, is owned by the new user
    // namespace, in which this process holds every capability until it
    // execs, so it may configure the namespace's loopback interface below.
    let flags = unshare_flags(mount, isolate_network);
    // SAFETY: Unshare does not have any unsafe effects and modifies no
    // memory, it is also async-signal-safe.
    if unsafe { libc::unshare(flags) } != 0 {
        return Err(Error::last_os_error());
    }

    if let Err(e) = write_signal_safe(c"/proc/self/setgroups", b"deny") {
        // If we fail to write this it will just make gid_map fail later,
        // but we may be able to continue anyway.
        if e != libc::EPERM && e != libc::EACCES && e != libc::ENOENT {
            return Err(Error::from_raw_os_error(e));
        }
    }

    let mut buffer = [0u8; 32];
    write_signal_safe(c"/proc/self/uid_map", create_map_line(uid, &mut buffer))
        .map_err(Error::from_raw_os_error)?;

    // If we can't write to gid_map, we just ignore it. This usually happens if
    // setgroups was not written to (because of permissions) or if we are in a
    // restricted environment.
    if let Err(e) = write_signal_safe(c"/proc/self/gid_map", create_map_line(gid, &mut buffer)) {
        // If this fails then we can probably continue just fine, it's just
        // the uid that's important.
        if e != libc::EPERM && e != libc::EACCES {
            return Err(Error::from_raw_os_error(e));
        }
    }

    // Configure the mount namespace if enabled.
    if mount {
        perform_remount(tmp_directory, root_action_directory, action_directory)?;
    }

    // An isolated action keeps loopback, so its own local servers work.
    if isolate_network {
        bring_up_loopback()?;
    }

    // Set hostname to "nativelink" to ensure reproducibility.
    let hostname = b"nativelink";
    // SAFETY: We reference the static memory above only and this is
    // async-signal-safe.
    if unsafe { libc::sethostname(hostname.as_ptr().cast(), hostname.len()) } != 0 {
        // SAFETY: We just called a libc function that failed.
        let err = unsafe { *libc::__errno_location() };
        if err != libc::EPERM && err != libc::EACCES {
            return Err(Error::from_raw_os_error(err));
        }
    }

    // Fork to enter the PID namespace.
    // SAFETY: We are already in a required async-signal-safe environment, we
    // will continue to ensure that ongoing.
    match unsafe { libc::fork() } {
        0 => {
            // SAFETY: This function is async-signal-safe and references no memory or resources.
            if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0 {
                exit(1);
            }
            Ok(())
        }
        pid if pid > 0 => {
            // Ensure that any children spawned by the action are re-parented to
            // this process if their parent dies. This is effectively a sub-reaper.
            // SAFETY: prctl is async-signal-safe.
            unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };

            // SAFETY: All operations below simply _exit and therefore there
            // are no issues with dangling file descriptor handles.
            unsafe { close_all_fds() };

            let mut sigset = core::mem::MaybeUninit::<libc::sigset_t>::uninit();
            // SAFETY: sigset is on the stack and we are initializing it.
            unsafe {
                libc::sigemptyset(sigset.as_mut_ptr());
                libc::sigaddset(sigset.as_mut_ptr(), libc::SIGTERM);
                libc::sigaddset(sigset.as_mut_ptr(), libc::SIGCHLD);
                libc::sigprocmask(libc::SIG_BLOCK, sigset.as_ptr(), core::ptr::null_mut());
            }

            loop {
                // Reap all exited children.
                loop {
                    let mut status = 0;
                    // SAFETY: The status is on the stack and waitpid is otherwise
                    // safe to call.
                    let res = unsafe { libc::waitpid(-1, &raw mut status, libc::WNOHANG) };
                    if res == pid {
                        if libc::WIFEXITED(status) {
                            exit(libc::WEXITSTATUS(status));
                        } else if libc::WIFSIGNALED(status) {
                            // Try to exit with the same signal as the child.
                            // SAFETY: The sigset was previously allocated and used on the stack.
                            unsafe {
                                libc::sigprocmask(
                                    libc::SIG_UNBLOCK,
                                    sigset.as_ptr(),
                                    core::ptr::null_mut(),
                                )
                            };
                            // SAFETY: It's always safe to raise and as a fallback we _exit below.
                            unsafe { libc::raise(libc::WTERMSIG(status)) };
                            // We shouldn't get here, but it's a fallback in case.
                            exit(libc::WTERMSIG(status));
                        }
                    } else if res <= 0 {
                        // SAFETY: We just called a libc function that failed.
                        if res == -1 && unsafe { *libc::__errno_location() } != libc::EINTR {
                            exit(255);
                        }
                        // Break the reaping loop to wait for signals.
                        break;
                    }
                }

                let mut siginfo = core::mem::MaybeUninit::<libc::siginfo_t>::uninit();
                // SAFETY: sigset is initialized and siginfo is on the stack.
                let sig = unsafe { libc::sigwaitinfo(sigset.as_ptr(), siginfo.as_mut_ptr()) };

                if sig == libc::SIGTERM {
                    // Pass the SIGTERM on, so the action gets the grace the
                    // worker's kill_grace_ms promises to write its own
                    // cleanup. The stub used to turn it into SIGKILL, which
                    // ended the action at once under namespaces while the
                    // same action got its grace without them. The hard kill
                    // is a SIGKILL to the stub itself: as this PID
                    // namespace's init, its death takes every process in
                    // the namespace with it.
                    // SAFETY: pid is valid and we are sending a signal.
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                }
            }
        }
        _ => Err(Error::last_os_error()),
    }
}
