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

/// A wrapper around a Child to send SIGTERM to kill the process instead
/// of SIGKILL as it's wrapped by the stub.
#[derive(Debug)]
pub struct MaybeNamespacedChild {
    namespaced: bool,
    child: tokio::process::Child,
}

impl MaybeNamespacedChild {
    pub const fn new(namespaced: bool, child: tokio::process::Child) -> Self {
        Self { namespaced, child }
    }

    /// Send SIGTERM if namespaced which sends SIGKILL to the child, otherwise
    /// send SIGKILL to the child.
    pub async fn kill(&mut self) -> Result<(), Error> {
        if self.namespaced {
            // It would be safer to call send_signal to use the pidfd to avoid
            // races, however this is still an experimental API, see:
            // https://github.com/rust-lang/rust/issues/141975
            // self.child.std_child().send_signal(Signal::SIGTERM)?;
            // return self.child.wait().await.map(|_| ());
            if let Some(pid) = self.child.id() {
                // SAFETY: pid is valid as provided by the wrapper and we are
                // sending a signal to the namespaced stub.
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                return self.child.wait().await.map(|_| ());
            }
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

enum NamespaceErrorType {
    Unshare = 1,
    WriteSignalSafe,
    Mount,
}

const NS_ERROR_TYPE_BITS: u8 = 2; // This is 2 because the highest value (NamespaceErrorType::Mount) is 3 and so we can store all of this in two bits
const NS_ERROR_TYPE_MASK: i32 = 0x3; // 11 - i.e. NS_ERROR_TYPE_BITS lowest bits

/// Determines whether the namespaces provided by this module are supported
/// on the currently running system by forking a process and trying to enter
/// it into the new namespaces.
pub fn namespaces_supported(mount: bool) -> bool {
    // SAFETY: Posix requires that geteuid is always successful.
    let uid = unsafe { libc::geteuid() };
    let uid_map = format!("{uid} {uid} 1\n");
    // SAFETY: We ensure that if pid == 0 we only call async-signal-safe functions.
    let pid = unsafe { libc::fork() };
    match pid {
        0 => {
            let mut flags =
                libc::CLONE_NEWPID | libc::CLONE_NEWUSER | libc::CLONE_NEWIPC | libc::CLONE_NEWUTS;
            if mount {
                flags |= libc::CLONE_NEWNS;
            }
            // SAFETY: Unshare does not have any unsafe effects and modifies no
            // memory, it is also async-signal-safe.
            if unsafe { libc::unshare(flags) } == 0 {
                match write_signal_safe(c"/proc/self/uid_map", uid_map.as_bytes()) {
                    Ok(()) => {
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
                        } == 0
                        {
                            exit(0);
                        } else {
                            // SAFETY: We just called a libc function that failed (-1).
                            let errno = unsafe { *libc::__errno_location() };
                            exit(
                                (NamespaceErrorType::Mount as i32) | (errno << NS_ERROR_TYPE_BITS),
                            );
                        }
                    }
                    Err(uid_map_err) => {
                        exit(
                            (NamespaceErrorType::WriteSignalSafe as i32)
                                | (uid_map_err << NS_ERROR_TYPE_BITS),
                        );
                    }
                }
            } else {
                // SAFETY: We just called a libc function that failed (-1).
                let errno = unsafe { *libc::__errno_location() };
                exit((NamespaceErrorType::Unshare as i32) | (errno << NS_ERROR_TYPE_BITS));
            }
        }
        pid if pid > 0 => {
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
            if libc::WIFEXITED(status) {
                match libc::WEXITSTATUS(status) {
                    0 => {
                        return true;
                    }
                    s if s & NS_ERROR_TYPE_MASK == NamespaceErrorType::Unshare as i32 => {
                        let errno = s >> NS_ERROR_TYPE_BITS;
                        error!(errno, "Namespaces: Error during unshare");
                        if errno == libc::EPERM {
                            error!(
                                "If the worker is inside Docker, namespaces don't work unless it's a privileged container"
                            );
                        }
                    }
                    s if s & NS_ERROR_TYPE_MASK == NamespaceErrorType::WriteSignalSafe as i32 => {
                        error!(
                            errno = s >> NS_ERROR_TYPE_BITS,
                            "Namespaces: Error while writing to /proc/self/uid_map"
                        );
                    }
                    s if s & NS_ERROR_TYPE_MASK == NamespaceErrorType::Mount as i32 => {
                        error!(
                            errno = s >> NS_ERROR_TYPE_BITS,
                            "Failure to mount during namespace checking"
                        );
                    }
                    other => {
                        error!(
                            exit_code = other,
                            "Namespace check failure with unknown exit code"
                        );
                    }
                }
                return false;
            }
            error!(
                exit_code = status,
                "Namespaces: waitpid exit with non-exit code"
            );
            return false;
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

fn perform_remount(
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

    // Mask the root action directory with a tmpfs to ensure sibling directories aren't visible.
    // SAFETY: mount is async-signal-safe. The filesystem type and target are valid C-strings.
    if unsafe {
        libc::mount(
            c"tmpfs".as_ptr(),
            root_action_directory.as_ptr(),
            c"tmpfs".as_ptr(),
            0,
            core::ptr::null(),
        )
    } != 0
    {
        return Err(Error::last_os_error());
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
/// This function is async-signal-safe and has no external locks or
/// memory allocations.
pub fn configure_namespace(
    mount: bool,
    root_action_directory: &core::ffi::CStr,
    action_directory: &core::ffi::CStr,
) -> std::io::Result<()> {
    // SAFETY: It is always safe to call geteuid on Posix.
    let uid = unsafe { libc::geteuid() };
    // SAFETY: It is always safe to call getegid on Posix.
    let gid = unsafe { libc::getegid() };

    let mut flags =
        libc::CLONE_NEWPID | libc::CLONE_NEWUSER | libc::CLONE_NEWIPC | libc::CLONE_NEWUTS;
    if mount {
        flags |= libc::CLONE_NEWNS;
    }
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
        perform_remount(root_action_directory, action_directory).unwrap();
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
                    // SAFETY: pid is valid and we are sending a signal.
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
        }
        _ => Err(Error::last_os_error()),
    }
}
