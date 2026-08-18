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
/// `false`, and `create_dir_many` treats an existing directory as success so a
/// parents-first batch composes with directories a caller already made.
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

    // Parents-first, and `parent` already exists so the batch must tolerate it.
    let parent = dir.join("parent");
    fs::create_dir(&parent)?;
    let results = create_dir_many(vec![parent.clone(), parent.join("child")]).await?;
    assert!(
        results.iter().all(Result::is_ok),
        "an existing directory is not an error"
    );
    assert!(fs::exists(parent.join("child"))?);

    // A missing parent is a genuine failure, reported in place.
    let results = create_dir_many(vec![dir.join("no/such/parent")]).await?;
    assert!(results[0].is_err(), "missing parent must surface an error");

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
