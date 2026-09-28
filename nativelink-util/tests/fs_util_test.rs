use std::path::PathBuf;

use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
#[cfg(unix)]
use nativelink_util::fs_util::set_dir_writable_recursive;
use nativelink_util::fs_util::{
    CloneMethod, calculate_directory_size, hardlink_directory_tree, set_readonly_recursive,
};
use tempfile::TempDir;
use tokio::fs;
use tokio::io::AsyncWriteExt;

async fn create_test_directory() -> Result<(TempDir, PathBuf), Error> {
    let temp_dir = TempDir::new().err_tip(|| "Failed to create temp directory")?;
    let test_dir = temp_dir.path().join("test_src");

    fs::create_dir(&test_dir).await?;

    // Create a file
    let file1 = test_dir.join("file1.txt");
    let mut f = fs::File::create(&file1).await?;
    f.write_all(b"Hello, World!").await?;
    f.sync_all().await?;
    drop(f);

    // Create a subdirectory with a file
    let subdir = test_dir.join("subdir");
    fs::create_dir(&subdir).await?;

    let file2 = subdir.join("file2.txt");
    let mut f = fs::File::create(&file2).await?;
    f.write_all(b"Nested file").await?;
    f.sync_all().await?;
    drop(f);

    Ok((temp_dir, test_dir))
}

#[nativelink_test]
async fn test_hardlink_directory_tree() -> Result<(), Error> {
    let (temp_dir, src_dir) = create_test_directory().await?;
    let dst_dir = temp_dir.path().join("test_dst");

    // Hardlink the directory
    let method = hardlink_directory_tree(&src_dir, &dst_dir).await?;

    #[cfg(target_os = "macos")]
    assert_eq!(method, CloneMethod::Clonefile, "macOS should use clonefile");
    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        method,
        CloneMethod::Hardlink,
        "non-macOS should use per-file hardlinks"
    );

    // Verify structure
    assert!(dst_dir.join("file1.txt").exists());
    assert!(dst_dir.join("subdir").is_dir());
    assert!(dst_dir.join("subdir/file2.txt").exists());

    // Verify contents
    let content1 = fs::read_to_string(dst_dir.join("file1.txt")).await?;
    assert_eq!(content1, "Hello, World!");

    let content2 = fs::read_to_string(dst_dir.join("subdir/file2.txt")).await?;
    assert_eq!(content2, "Nested file");

    // Linux: per-file hardlinks share inodes with the source.
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::os::unix::fs::MetadataExt;
        let src_meta = fs::metadata(src_dir.join("file1.txt")).await?;
        let dst_meta = fs::metadata(dst_dir.join("file1.txt")).await?;
        assert_eq!(
            src_meta.ino(),
            dst_meta.ino(),
            "Files should have same inode (hardlinked)"
        );
    }

    // macOS: clonefile(2) creates distinct inodes that share data via COW.
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;
        let src_meta = fs::metadata(src_dir.join("file1.txt")).await?;
        let dst_meta = fs::metadata(dst_dir.join("file1.txt")).await?;
        assert_ne!(
            src_meta.ino(),
            dst_meta.ino(),
            "clonefile should create distinct inodes from source"
        );
    }

    Ok(())
}

#[cfg(target_os = "macos")]
#[nativelink_test]
async fn test_clonefile_dirs_writable_files_readonly() -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;

    let (temp_dir, src_dir) = create_test_directory().await?;
    // Source mimics a directory cache entry: writable dirs (0o755),
    // read-only files (0o555).
    set_readonly_recursive(&src_dir).await?;

    let dst_dir = temp_dir.path().join("clone_dst");
    hardlink_directory_tree(&src_dir, &dst_dir).await?;

    // Root: writable, so the worker can drop the action's declared
    // outputs inside it.
    let root_mode = fs::metadata(&dst_dir).await?.permissions().mode() & 0o777;
    assert_eq!(root_mode, 0o755, "destination root must be writable");

    // Nested subdir: writable too. `clonefile(2)` copies the source's
    // modes verbatim and the source's directories were left writable by
    // `set_readonly_recursive`. Bazel actions declare outputs at paths
    // nested inside input subdirectories, so every directory in the
    // materialized tree must be writable — no separate chmod walk needed.
    let dst_subdir_mode = fs::metadata(dst_dir.join("subdir"))
        .await?
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dst_subdir_mode, 0o755,
        "cloned subdirs must be writable so nested outputs can be created"
    );

    // Existing file: stays read-only. Hermeticity contract — inputs are
    // not writable. Matches Bazel's local-sandbox model and REAPI
    // Action.output_files semantics: actions can only write to declared
    // outputs, not mutate inputs.
    let dst_file_mode = fs::metadata(dst_dir.join("file1.txt"))
        .await?
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dst_file_mode, 0o555,
        "cloned files must inherit source read-only mode"
    );

    // Source untouched: dirs writable, files read-only.
    let src_subdir_mode = fs::metadata(src_dir.join("subdir"))
        .await?
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        src_subdir_mode, 0o755,
        "source dir should still be writable after clone"
    );
    let src_file_mode = fs::metadata(src_dir.join("file1.txt"))
        .await?
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        src_file_mode, 0o555,
        "source file should still be read-only after clone"
    );

    Ok(())
}

#[cfg(target_os = "macos")]
#[nativelink_test]
async fn test_clonefile_root_accepts_new_files() -> Result<(), Error> {
    let (temp_dir, src_dir) = create_test_directory().await?;
    set_readonly_recursive(&src_dir).await?;

    let dst_dir = temp_dir.path().join("clone_dst");
    hardlink_directory_tree(&src_dir, &dst_dir).await?;

    // The worker creates declared output files at the action's
    // working directory root. Verify a new file can be created there
    // even though everything inside the clone is read-only (0o555).
    let new_output = dst_dir.join("new_output.bin");
    fs::write(&new_output, b"action output").await?;
    assert_eq!(fs::read(&new_output).await?, b"action output");

    Ok(())
}

#[cfg(target_os = "macos")]
#[nativelink_test]
async fn test_clonefile_input_mutation_fails() -> Result<(), Error> {
    let (temp_dir, src_dir) = create_test_directory().await?;
    set_readonly_recursive(&src_dir).await?;

    let dst_dir = temp_dir.path().join("clone_dst");
    hardlink_directory_tree(&src_dir, &dst_dir).await?;

    // Hermeticity: actions cannot mutate inputs. A write to an input
    // file in the cloned tree must fail with EACCES, mirroring what
    // Bazel's linux-sandbox / darwin-sandbox would do.
    let input_file = dst_dir.join("file1.txt");
    let err = fs::write(&input_file, b"mutated")
        .await
        .expect_err("input file write should fail (file is 0o555, no write bit)");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    // Source must be untouched.
    let src_content = fs::read_to_string(src_dir.join("file1.txt")).await?;
    assert_eq!(src_content, "Hello, World!");

    Ok(())
}

#[cfg(target_os = "macos")]
#[nativelink_test]
async fn test_clonefile_cow_isolation() -> Result<(), Error> {
    let (temp_dir, src_dir) = create_test_directory().await?;
    let dst_dir = temp_dir.path().join("clone_dst");

    hardlink_directory_tree(&src_dir, &dst_dir).await?;

    // Mutate the clone and confirm the source is unaffected.
    let dst_file = dst_dir.join("file1.txt");
    fs::write(&dst_file, b"mutated by clone").await?;

    let src_content = fs::read_to_string(src_dir.join("file1.txt")).await?;
    assert_eq!(
        src_content, "Hello, World!",
        "source must be untouched after writing to clone (COW)"
    );

    let dst_content = fs::read_to_string(&dst_file).await?;
    assert_eq!(dst_content, "mutated by clone");

    Ok(())
}

/// Bazel actions declare outputs at paths nested inside input
/// subdirectories. Because `set_readonly_recursive` leaves directories
/// writable and `clonefile(2)` copies modes verbatim, the materialized
/// tree already accepts a nested output file with NO separate
/// `set_dir_writable_recursive` walk — that is the redundant work this
/// change removes from `prepare_action_inputs`.
#[cfg(target_os = "macos")]
#[nativelink_test]
async fn test_clonefile_nested_output_without_dir_writable_walk() -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;

    let (temp_dir, src_dir) = create_test_directory().await?;
    // Lock the source down the way the directory cache does after
    // constructing a cache entry: writable dirs, read-only files.
    set_readonly_recursive(&src_dir).await?;

    let dst_dir = temp_dir.path().join("clone_dst");
    hardlink_directory_tree(&src_dir, &dst_dir).await?;

    // Creating an output nested inside a cloned subdir succeeds straight
    // away — no recursive chmod walk. This is the post-condition that
    // lets `prepare_action_inputs` drop its `set_dir_writable_recursive`
    // call.
    let nested_output = dst_dir.join("subdir").join("nested_output.o");
    fs::write(&nested_output, b"action output").await?;
    assert_eq!(fs::read(&nested_output).await?, b"action output");

    // Files inside the tree stay read-only — hermeticity holds, and the
    // CAS-hardlink inode invariant is preserved.
    let file_mode = fs::metadata(dst_dir.join("subdir").join("file2.txt"))
        .await?
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o555, "input files must remain read-only");

    // A write to an input file still fails — actions cannot mutate inputs.
    let err = fs::write(dst_dir.join("subdir").join("file2.txt"), b"mutated")
        .await
        .expect_err("input file write must fail (file is 0o555)");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    Ok(())
}

/// `set_readonly_recursive` locks a tree down as a cache entry: every
/// file is made read-only, every directory is left writable. Directories
/// stay writable because they are not hardlink-shared between cache
/// entries, and a writable directory mode lets the materialized
/// destination tree accept nested action outputs without a separate
/// chmod walk.
#[nativelink_test]
async fn test_set_readonly_recursive() -> Result<(), Error> {
    let (_temp_dir, test_dir) = create_test_directory().await?;

    set_readonly_recursive(&test_dir).await?;

    // Files are read-only.
    let metadata = fs::metadata(test_dir.join("file1.txt")).await?;
    assert!(metadata.permissions().readonly());

    let metadata = fs::metadata(test_dir.join("subdir/file2.txt")).await?;
    assert!(metadata.permissions().readonly());

    // Directories are left writable — root and every nested subdir.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in [test_dir.clone(), test_dir.join("subdir")] {
            let mode = fs::metadata(&dir).await?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "{} must stay writable", dir.display());
        }
    }
    #[cfg(windows)]
    {
        // On Windows directories carry no read-only attribute that would
        // block creating children; assert they are not marked read-only.
        for dir in [test_dir.clone(), test_dir.join("subdir")] {
            assert!(
                !fs::metadata(&dir).await?.permissions().readonly(),
                "{} must stay writable",
                dir.display()
            );
        }
    }

    Ok(())
}

/// `set_dir_writable_recursive` must make *every* directory in a tree
/// writable — including nested subdirs — so the eviction cleanup path can
/// `remove_dir_all` a cache entry. Files are left read-only because they
/// may share a CAS inode via hardlink. This walk runs on already-read-only
/// directory trees too, so the test first sets every file read-only with
/// `set_readonly_recursive`.
#[cfg(unix)]
#[nativelink_test]
async fn test_set_dir_writable_recursive_walks_nested_dirs() -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;

    let (_temp_dir, test_dir) = create_test_directory().await?;
    // Lock files down, then explicitly force every directory read-only so
    // the walk has real work to do (the directory cache leaves dirs
    // writable, but the eviction path must cope with any mode).
    set_readonly_recursive(&test_dir).await?;
    for dir in [test_dir.clone(), test_dir.join("subdir")] {
        fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).await?;
    }

    set_dir_writable_recursive(&test_dir).await?;

    // Every directory — the root and the nested subdir — must be writable.
    for dir in [test_dir.clone(), test_dir.join("subdir")] {
        let mode = fs::metadata(&dir).await?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "{} must be writable", dir.display());
    }

    // Files stay read-only — chmoding them would corrupt a shared CAS inode.
    let file_mode = fs::metadata(test_dir.join("subdir/file2.txt"))
        .await?
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o555, "files must remain read-only");

    Ok(())
}

/// Regression test for the directory-cache fallback bug: input trees
/// produced by `rules_python` / `rules_apple` venv tooling contain
/// symlinks (e.g. `.venv/bin/python3`). `set_readonly_recursive` walks the
/// materialized tree with `chmod`; `chmod` follows symlinks, so a symlink
/// to a file would mutate the target and a *dangling* symlink (target
/// outside the action's input set) would fail the whole walk with ENOENT
/// — pushing the action onto the slow `download_to_directory` fallback.
/// The walk must `lstat` and skip the symlink, leaving it intact.
#[cfg(unix)]
#[nativelink_test]
async fn test_set_readonly_recursive_skips_symlinks() -> Result<(), Error> {
    let (_temp_dir, test_dir) = create_test_directory().await?;

    // A symlink to a path *inside* the same tree (the realistic
    // `.venv/bin/python3 -> ../../file1.txt` shape).
    let internal_link = test_dir.join("link_to_file1");
    fs::symlink("file1.txt", &internal_link).await?;

    // A symlink with a *relative* target that does not resolve (dangling).
    // This is the case that previously failed the walk with ENOENT.
    let dangling_link = test_dir.join("dangling_link");
    fs::symlink("../does/not/exist", &dangling_link).await?;

    // A symlink that points at a directory inside the tree. With `stat`
    // the walk would recurse *through* this link; with `lstat` it must
    // not.
    let dir_link = test_dir.join("link_to_subdir");
    fs::symlink("subdir", &dir_link).await?;

    // The walk must succeed despite the symlinks.
    set_readonly_recursive(&test_dir).await?;

    // Every symlink is preserved as a symlink with its target intact.
    for (link, expected_target) in [
        (&internal_link, "file1.txt"),
        (&dangling_link, "../does/not/exist"),
        (&dir_link, "subdir"),
    ] {
        let link_meta = fs::symlink_metadata(link).await?;
        assert!(
            link_meta.is_symlink(),
            "{} must still be a symlink after the walk",
            link.display()
        );
        assert_eq!(
            fs::read_link(link).await?,
            PathBuf::from(expected_target),
            "{} target must be unchanged",
            link.display()
        );
    }

    // The real files were still made read-only.
    assert!(
        fs::metadata(test_dir.join("file1.txt"))
            .await?
            .permissions()
            .readonly()
    );

    Ok(())
}

/// Companion to the read-only test: `set_dir_writable_recursive` must also
/// be symlink-safe. It must not `chmod` a symlink (which would follow the
/// link) and must not recurse through a symlinked directory.
#[cfg(unix)]
#[nativelink_test]
async fn test_set_dir_writable_recursive_skips_symlinks() -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;

    let (_temp_dir, test_dir) = create_test_directory().await?;

    // Symlink to a file inside the tree, a dangling relative symlink, and
    // a symlink pointing at a directory inside the tree.
    fs::symlink("file1.txt", test_dir.join("link_to_file1")).await?;
    fs::symlink("../does/not/exist", test_dir.join("dangling_link")).await?;
    fs::symlink("subdir", test_dir.join("link_to_subdir")).await?;

    // Mirror the directory cache's post-construction sequence.
    set_readonly_recursive(&test_dir).await?;
    set_dir_writable_recursive(&test_dir).await?;

    // Symlinks survive both walks untouched.
    for (link, expected_target) in [
        ("link_to_file1", "file1.txt"),
        ("dangling_link", "../does/not/exist"),
        ("link_to_subdir", "subdir"),
    ] {
        let link_path = test_dir.join(link);
        assert!(
            fs::symlink_metadata(&link_path).await?.is_symlink(),
            "{} must still be a symlink",
            link_path.display()
        );
        assert_eq!(
            fs::read_link(&link_path).await?,
            PathBuf::from(expected_target),
            "{} target must be unchanged",
            link_path.display()
        );
    }

    // Real directories were made writable; real files stayed read-only.
    let dir_mode = fs::metadata(test_dir.join("subdir"))
        .await?
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o755, "real subdir must be writable");
    let file_mode = fs::metadata(test_dir.join("subdir/file2.txt"))
        .await?
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o555, "real files must stay read-only");

    Ok(())
}

/// `hardlink_directory_tree` must recreate symlink entries as symlinks at
/// the destination (not dereference them), and the subsequent
/// `set_readonly_recursive` walk over the materialized tree must succeed.
/// This is the end-to-end shape `DirectoryCache::get_or_create` runs.
#[cfg(unix)]
#[nativelink_test]
async fn test_hardlink_directory_tree_preserves_symlinks() -> Result<(), Error> {
    let (temp_dir, src_dir) = create_test_directory().await?;

    // Symlink to a sibling file, a dangling relative symlink, and a
    // symlink to a subdirectory — all inside the source tree.
    fs::symlink("file1.txt", src_dir.join("link_to_file1")).await?;
    fs::symlink("../does/not/exist", src_dir.join("dangling_link")).await?;
    fs::symlink("subdir", src_dir.join("link_to_subdir")).await?;

    let dst_dir = temp_dir.path().join("test_dst");
    hardlink_directory_tree(&src_dir, &dst_dir).await?;

    // Each symlink is materialized as a symlink with its target intact.
    for (link, expected_target) in [
        ("link_to_file1", "file1.txt"),
        ("dangling_link", "../does/not/exist"),
        ("link_to_subdir", "subdir"),
    ] {
        let link_path = dst_dir.join(link);
        assert!(
            fs::symlink_metadata(&link_path).await?.is_symlink(),
            "{} must be a symlink in the materialized tree",
            link_path.display()
        );
        assert_eq!(
            fs::read_link(&link_path).await?,
            PathBuf::from(expected_target),
            "{} target must be preserved",
            link_path.display()
        );
    }

    // The read-only walk over the materialized tree must not choke on the
    // symlinks (this is the operation that previously failed the cache).
    set_readonly_recursive(&dst_dir).await?;

    Ok(())
}

#[nativelink_test]
async fn test_calculate_directory_size() -> Result<(), Error> {
    let (_temp_dir, test_dir) = create_test_directory().await?;

    let size = calculate_directory_size(&test_dir).await?;

    // "Hello, World!" = 13 bytes
    // "Nested file" = 11 bytes
    // Total = 24 bytes
    assert_eq!(size, 24);

    Ok(())
}

#[nativelink_test]
async fn test_hardlink_nonexistent_source() {
    let temp_dir = TempDir::new().unwrap();
    let src = temp_dir.path().join("nonexistent");
    let dst = temp_dir.path().join("dest");

    let result = hardlink_directory_tree(&src, &dst).await;
    assert!(result.is_err());
}

#[nativelink_test]
async fn test_hardlink_existing_destination() -> Result<(), Error> {
    let (temp_dir, src_dir) = create_test_directory().await?;
    let dst_dir = temp_dir.path().join("existing");

    fs::create_dir(&dst_dir).await?;

    let result = hardlink_directory_tree(&src_dir, &dst_dir).await;
    assert!(result.is_err());

    Ok(())
}
