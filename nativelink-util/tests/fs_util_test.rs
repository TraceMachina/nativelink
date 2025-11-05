// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::PathBuf;

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_util::fs_util::{
    calculate_directory_size, hardlink_directory_tree, set_readonly_recursive,
    set_writable_recursive,
};
use tempfile::TempDir;
use tokio::fs;
use tokio::io::AsyncWriteExt;

async fn create_test_directory() -> Result<(TempDir, PathBuf), Error> {
    let temp_dir = TempDir::new().map_err(|e| {
        nativelink_error::make_err!(
            nativelink_error::Code::Internal,
            "Failed to create temp directory: {e}"
        )
    })?;
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
async fn hardlink_directory_tree_test() -> Result<(), Error> {
    let (_temp_dir, src_dir) = create_test_directory().await?;
    let dst_dir = _temp_dir.path().join("test_dst");

    // Hardlink the directory
    hardlink_directory_tree(&src_dir, &dst_dir).await?;

    // Verify structure
    assert!(dst_dir.join("file1.txt").exists());
    assert!(dst_dir.join("subdir").is_dir());
    assert!(dst_dir.join("subdir/file2.txt").exists());

    // Verify contents
    let content1 = fs::read_to_string(dst_dir.join("file1.txt")).await?;
    assert_eq!(content1, "Hello, World!");

    let content2 = fs::read_to_string(dst_dir.join("subdir/file2.txt")).await?;
    assert_eq!(content2, "Nested file");

    // Verify files are hardlinked (same inode on Unix)
    #[cfg(unix)]
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

    Ok(())
}

#[nativelink_test]
async fn set_readonly_recursive_test() -> Result<(), Error> {
    let (_temp_dir, test_dir) = create_test_directory().await?;

    set_readonly_recursive(&test_dir).await?;

    // Verify files are read-only
    let metadata = fs::metadata(test_dir.join("file1.txt")).await?;
    assert!(metadata.permissions().readonly());

    let metadata = fs::metadata(test_dir.join("subdir/file2.txt")).await?;
    assert!(metadata.permissions().readonly());

    Ok(())
}

#[nativelink_test]
async fn set_writable_recursive_test() -> Result<(), Error> {
    let (_temp_dir, test_dir) = create_test_directory().await?;

    // First set to read-only
    set_readonly_recursive(&test_dir).await?;

    // Verify read-only
    let metadata = fs::metadata(test_dir.join("file1.txt")).await?;
    assert!(metadata.permissions().readonly());

    // Now set to writable
    set_writable_recursive(&test_dir).await?;

    // Verify writable (not read-only)
    let metadata = fs::metadata(test_dir.join("file1.txt")).await?;
    assert!(!metadata.permissions().readonly());

    let metadata = fs::metadata(test_dir.join("subdir/file2.txt")).await?;
    assert!(!metadata.permissions().readonly());

    Ok(())
}

#[nativelink_test]
async fn calculate_directory_size_test() -> Result<(), Error> {
    let (_temp_dir, test_dir) = create_test_directory().await?;

    let size = calculate_directory_size(&test_dir).await?;

    // "Hello, World!" = 13 bytes
    // "Nested file" = 11 bytes
    // Total = 24 bytes
    assert_eq!(size, 24);

    Ok(())
}

#[nativelink_test]
async fn hardlink_nonexistent_source_test() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let src = temp_dir.path().join("nonexistent");
    let dst = temp_dir.path().join("dest");

    let result = hardlink_directory_tree(&src, &dst).await;
    assert!(result.is_err());

    Ok(())
}

#[nativelink_test]
async fn hardlink_existing_destination_test() -> Result<(), Error> {
    let (_temp_dir, src_dir) = create_test_directory().await?;
    let dst_dir = _temp_dir.path().join("existing");

    // Create the destination directory (should now work)
    fs::create_dir(&dst_dir).await?;

    // This should now succeed since we allow pre-existing directories
    hardlink_directory_tree(&src_dir, &dst_dir).await?;

    // Verify the files were hardlinked
    assert!(dst_dir.join("file1.txt").exists());
    assert!(dst_dir.join("subdir").is_dir());
    assert!(dst_dir.join("subdir/file2.txt").exists());

    Ok(())
}
