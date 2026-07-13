#![cfg(not(target_family = "windows"))]
// Because windows does permissions differently

use std::env;
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;

use nativelink_error::ResultExt;
use nativelink_macro::nativelink_test;
use nativelink_util::fs::remove_dir_all;

#[nativelink_test]
async fn remove_files_with_bad_permissions() -> Result<(), Box<dyn core::error::Error>> {
    let temp_dir = env::temp_dir();
    let bad_perms_directory = temp_dir.join("bad_perms_directory");
    if fs::exists(&bad_perms_directory)? {
        remove_dir_all(&bad_perms_directory)
            .await
            .err_tip(|| format!("first remove_dir_all for {bad_perms_directory:?}"))?;
    }
    fs::create_dir(&bad_perms_directory)?;
    let bad_perms_file = bad_perms_directory.join("bad_perms_file");
    if !fs::exists(&bad_perms_file)? {
        fs::write(&bad_perms_file, "").err_tip(|| "Can't create file")?;
    }

    fs::set_permissions(&bad_perms_directory, Permissions::from_mode(0o100)) // execute owner only
        .err_tip(|| "Can't set perms on directory")?;

    fs::set_permissions(&bad_perms_file, Permissions::from_mode(0o400)) // read owner only
        .err_tip(|| "Can't set perms on file")?;

    remove_dir_all(&bad_perms_directory)
        .await
        .err_tip(|| format!("second remove_dir_all for {bad_perms_directory:?}"))?;

    assert!(!fs::exists(&bad_perms_directory)?);
    Ok(())
}

#[cfg(target_os = "linux")]
#[nativelink_test]
async fn freebind_allows_binding_unassigned_address() -> Result<(), Box<dyn core::error::Error>> {
    use std::io::ErrorKind;

    use nativelink_util::fs::set_freebind;
    use tokio::net::TcpSocket;

    let addr = "192.0.2.1:0".parse()?;

    // Without `IP_FREEBIND` the kernel refuses to bind an unassigned address.
    let err = TcpSocket::new_v4()?.bind(addr).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::AddrNotAvailable);

    // With IP_FREEBIND the same bind succeeds.
    let socket = TcpSocket::new_v4()?;
    set_freebind(&socket)?;
    socket.bind(addr)?;

    Ok(())
}

// Regression test: `fs::read_dir` must complete with a single blocking-pool
// thread. The pre-fix implementation `block_on`ed `tokio::fs::read_dir`
// (itself a `spawn_blocking`) from inside a blocking-pool thread, so each
// call needed two pool threads at once; enough concurrent callers parked
// every thread on inner tasks that could never run, freezing all `fs::` ops
// process-wide. On a one-thread pool the old code deadlocks and the timeout
// below fires.
#[test]
fn read_dir_needs_only_one_blocking_thread() -> Result<(), Box<dyn core::error::Error>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test needs a runtime with a one-thread blocking pool"
    )]
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()?;
    rt.block_on(async {
        let read_dir = tokio::time::timeout(
            core::time::Duration::from_secs(5),
            nativelink_util::fs::read_dir(env::temp_dir()),
        )
        .await
        .expect("read_dir deadlocked: it required a second blocking-pool thread")?;
        drop(read_dir);
        Ok(())
    })
}
