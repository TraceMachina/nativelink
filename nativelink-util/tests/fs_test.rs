#![cfg(not(target_family = "windows"))]
// Because windows does permissions differently

use std::env;
use std::fs::{self, Permissions};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use nativelink_error::ResultExt;
use nativelink_macro::nativelink_test;
use nativelink_util::fs::{create_dir_many, exists_many, hard_link_many, remove_dir_all};

/// `hard_link_many` trades one `spawn_blocking` dispatch per link for one per
/// batch, so its whole contract is that the batch still behaves like N
/// independent `hard_link` calls: real hardlinks (shared inode, not copies),
/// one result per input, positionally aligned, with a failing link neither
/// aborting the batch nor shifting its neighbours' results.
#[nativelink_test]
async fn hard_link_many_links_in_order_and_isolates_failures()
-> Result<(), Box<dyn core::error::Error>> {
    let dir = env::temp_dir().join("hard_link_many_test");
    drop(remove_dir_all(&dir).await);
    fs::create_dir_all(&dir)?;

    let (src_a, src_b) = (dir.join("a.src"), dir.join("b.src"));
    fs::write(&src_a, "a")?;
    fs::write(&src_b, "b")?;

    // Middle entry's source does not exist, so only it may fail.
    let links = vec![
        (src_a.clone(), dir.join("a.dst")),
        (dir.join("missing.src"), dir.join("missing.dst")),
        (src_b.clone(), dir.join("b.dst")),
    ];
    let results = hard_link_many(links).await?;

    assert_eq!(results.len(), 3, "one result per input");
    assert!(results[0].is_ok(), "first link should succeed");
    assert!(results[1].is_err(), "missing source should fail");
    assert!(
        results[2].is_ok(),
        "a failed link must not abort the rest of the batch"
    );

    // A hardlink shares the source inode; a copy would not.
    for (src, dst) in [(&src_a, "a.dst"), (&src_b, "b.dst")] {
        assert_eq!(
            fs::metadata(src)?.ino(),
            fs::metadata(dir.join(dst))?.ino(),
            "{dst} should share an inode with its source",
        );
    }
    assert!(!fs::exists(dir.join("missing.dst"))?);

    assert!(
        hard_link_many(Vec::new()).await?.is_empty(),
        "empty batch is a no-op"
    );

    remove_dir_all(&dir).await?;
    Ok(())
}

/// `exists_many` answers positionally and reports every failure mode as
/// `false`, and `create_dir_many` creates parents before children while
/// refusing to swallow a directory that already exists.
#[nativelink_test]
async fn exists_many_and_create_dir_many_answer_positionally()
-> Result<(), Box<dyn core::error::Error>> {
    let dir = env::temp_dir().join("exists_many_test");
    drop(remove_dir_all(&dir).await);
    fs::create_dir_all(&dir)?;

    let present = dir.join("present");
    fs::write(&present, "x")?;

    let answers = exists_many(vec![
        dir.join("absent"),
        present.clone(),
        dir.join("nonexistent-parent/child"),
    ])
    .await?;
    assert_eq!(
        answers,
        vec![false, true, false],
        "answers must line up with the paths asked about"
    );
    assert!(exists_many(Vec::new()).await?.is_empty());

    // A child may be listed before its parent; the batch sorts them itself.
    let parent = dir.join("parent");
    create_dir_many(vec![parent.join("child"), parent.clone()]).await?;
    assert!(fs::exists(parent.join("child"))?);

    // An existing directory is an error, not silent success. Tolerating it
    // would merge two trees into one wherever sibling names collide, which is
    // any pair differing only in case on a case-insensitive filesystem.
    assert!(
        create_dir_many(vec![parent.clone()]).await.is_err(),
        "an already-existing directory must surface an error"
    );

    assert!(
        create_dir_many(vec![dir.join("no/such/parent")])
            .await
            .is_err(),
        "missing parent must surface an error"
    );

    remove_dir_all(&dir).await?;
    Ok(())
}

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
#[expect(
    clippy::disallowed_methods,
    reason = "test needs a runtime with a one-thread blocking pool; no util wrapper exposes max_blocking_threads"
)]
fn read_dir_needs_only_one_blocking_thread() -> Result<(), Box<dyn core::error::Error>> {
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
