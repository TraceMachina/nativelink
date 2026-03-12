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

use std::path::Path;

use nativelink_error::{Error, make_err};

/// Copies an entire directory tree from source to destination using the
/// fastest available method:
///
/// - **macOS (APFS)**: Uses `clonefile(2)` for a CoW clone of the entire tree
///   in a single syscall (~1ms regardless of tree size). Falls back to hardlink
///   if clonefile fails (cross-device, non-APFS, etc.).
/// - **Other platforms**: Hardlinks each file individually via `std::fs::hard_link`.
///
/// After a successful clonefile, directories are made writable (0o755) since the
/// clone inherits the cache's read-only permissions and actions need to create
/// output files.
pub async fn hardlink_directory_tree(src_dir: &Path, dst_dir: &Path) -> Result<(), Error> {
    let src = src_dir.to_path_buf();
    let dst = dst_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        {
            match try_clonefile(&src, &dst) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::debug!(
                        src = %src.display(),
                        dst = %dst.display(),
                        "clonefile failed, falling back to hardlink: {e}",
                    );
                }
            }
        }
        hardlink_directory_tree_sync(&src, &dst)
    })
    .await
    .map_err(|e| make_err!(nativelink_error::Code::Internal, "spawn_blocking join error: {e}"))?
}

/// Uses macOS `clonefile(2)` to CoW-clone an entire directory tree in one syscall.
/// Handles pre-existing (empty) destination by removing it first.
/// After cloning, makes all directories writable (0o755) since the clone
/// inherits the cache's read-only (0o555) permissions.
#[cfg(target_os = "macos")]
fn try_clonefile(src: &std::path::Path, dst: &std::path::Path) -> Result<(), Error> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    extern "C" {
        fn clonefile(
            src: *const std::ffi::c_char,
            dst: *const std::ffi::c_char,
            flags: std::ffi::c_int,
        ) -> std::ffi::c_int;
    }

    if !src.exists() {
        return Err(make_err!(
            nativelink_error::Code::InvalidArgument,
            "Source directory does not exist: {}",
            src.display()
        ));
    }

    let src_c = CString::new(src.as_os_str().as_bytes()).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Invalid src path for clonefile: {e}"
        )
    })?;
    let dst_c = CString::new(dst.as_os_str().as_bytes()).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Invalid dst path for clonefile: {e}"
        )
    })?;

    // clonefile(2) requires the destination to not exist.
    // The work directory may have been pre-created — remove it first.
    if dst.exists() {
        std::fs::remove_dir(dst).map_err(|e| {
            make_err!(
                nativelink_error::Code::Internal,
                "Failed to remove existing dst for clonefile {}: {e}",
                dst.display()
            )
        })?;
    }

    // SAFETY: src_c and dst_c are valid CStrings with nul terminators.
    let ret = unsafe { clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        return Err(make_err!(
            nativelink_error::Code::Internal,
            "clonefile {} → {}: {err}",
            src.display(),
            dst.display()
        ));
    }

    // The clone inherits the cache's read-only directory permissions (0o555).
    // Actions need writable directories to create output files, so walk the
    // cloned tree and make all directories writable.
    make_dirs_writable_sync(dst)?;

    Ok(())
}

/// Recursively makes all directories in a tree writable (0o755).
/// Only touches directories — files keep their existing permissions.
#[cfg(target_os = "macos")]
fn make_dirs_writable_sync(path: &std::path::Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::symlink_metadata(path).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Failed to get metadata for {}: {e}",
            path.display()
        )
    })?;

    if !metadata.is_dir() {
        return Ok(());
    }

    // Make this directory writable
    let mut perms = metadata.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Failed to chmod directory {}: {e}",
            path.display()
        )
    })?;

    // Recurse into subdirectories only (skip files and symlinks)
    for entry in std::fs::read_dir(path).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Failed to read directory {}: {e}",
            path.display()
        )
    })? {
        let entry = entry.map_err(|e| {
            make_err!(
                nativelink_error::Code::Internal,
                "Failed to read entry in {}: {e}",
                path.display()
            )
        })?;
        let ft = entry.file_type().map_err(|e| {
            make_err!(
                nativelink_error::Code::Internal,
                "Failed to get file type for {:?}: {e}",
                entry.path()
            )
        })?;
        if ft.is_dir() {
            make_dirs_writable_sync(&entry.path())?;
        }
    }

    Ok(())
}

/// Synchronous recursive hardlink — runs inside `spawn_blocking`.
fn hardlink_directory_tree_sync(src: &Path, dst: &Path) -> Result<(), Error> {
    if !src.exists() {
        return Err(make_err!(
            nativelink_error::Code::InvalidArgument,
            "Source directory does not exist: {}",
            src.display()
        ));
    }
    std::fs::create_dir_all(dst).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Failed to create destination directory {}: {e}",
            dst.display()
        )
    })?;
    hardlink_recursive_sync(src, dst)
}

fn hardlink_recursive_sync(src: &Path, dst: &Path) -> Result<(), Error> {
    for entry in std::fs::read_dir(src).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Failed to read directory {}: {e}",
            src.display()
        )
    })? {
        let entry = entry.map_err(|e| {
            make_err!(
                nativelink_error::Code::Internal,
                "Failed to read entry in {}: {e}",
                src.display()
            )
        })?;
        let ft = entry.file_type().map_err(|e| {
            make_err!(
                nativelink_error::Code::Internal,
                "Failed to get file type for {:?}: {e}",
                entry.path()
            )
        })?;
        let dst_path = dst.join(entry.file_name());

        if ft.is_dir() {
            std::fs::create_dir(&dst_path).map_err(|e| {
                make_err!(
                    nativelink_error::Code::Internal,
                    "Failed to create directory {}: {e}",
                    dst_path.display()
                )
            })?;
            hardlink_recursive_sync(&entry.path(), &dst_path)?;
        } else if ft.is_file() {
            std::fs::hard_link(entry.path(), &dst_path).map_err(|e| {
                make_err!(
                    nativelink_error::Code::Internal,
                    "Failed to hardlink {} to {}: {e}",
                    entry.path().display(),
                    dst_path.display()
                )
            })?;
        } else if ft.is_symlink() {
            let target = std::fs::read_link(entry.path()).map_err(|e| {
                make_err!(
                    nativelink_error::Code::Internal,
                    "Failed to read symlink {:?}: {e}",
                    entry.path()
                )
            })?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dst_path).map_err(|e| {
                make_err!(
                    nativelink_error::Code::Internal,
                    "Failed to create symlink {}: {e}",
                    dst_path.display()
                )
            })?;
            #[cfg(windows)]
            {
                if target.is_dir() {
                    std::os::windows::fs::symlink_dir(&target, &dst_path).map_err(|e| {
                        make_err!(
                            nativelink_error::Code::Internal,
                            "Failed to create dir symlink {}: {e}",
                            dst_path.display()
                        )
                    })?;
                } else {
                    std::os::windows::fs::symlink_file(&target, &dst_path).map_err(|e| {
                        make_err!(
                            nativelink_error::Code::Internal,
                            "Failed to create file symlink {}: {e}",
                            dst_path.display()
                        )
                    })?;
                }
            }
        }
    }
    Ok(())
}

/// Sets a directory tree to read-only recursively.
///
/// Uses `spawn_blocking` with synchronous `std::fs` for performance.
pub async fn set_readonly_recursive(dir: &Path) -> Result<(), Error> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || set_readonly_recursive_sync(&dir))
        .await
        .map_err(|e| make_err!(nativelink_error::Code::Internal, "spawn_blocking join error: {e}"))?
}

fn set_readonly_recursive_sync(path: &Path) -> Result<(), Error> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Failed to get metadata for {}: {e}",
            path.display()
        )
    })?;

    if metadata.is_symlink() {
        return Ok(());
    }

    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|e| {
            make_err!(
                nativelink_error::Code::Internal,
                "Failed to read directory {}: {e}",
                path.display()
            )
        })? {
            let entry = entry.map_err(|e| {
                make_err!(
                    nativelink_error::Code::Internal,
                    "Failed to read entry in {}: {e}",
                    path.display()
                )
            })?;
            set_readonly_recursive_sync(&entry.path())?;
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = metadata.permissions();
        let mode = perms.mode() & !0o222;
        perms.set_mode(mode);
        std::fs::set_permissions(path, perms).map_err(|e| {
            make_err!(
                nativelink_error::Code::Internal,
                "Failed to set permissions for {}: {e}",
                path.display()
            )
        })?;
    }

    #[cfg(windows)]
    {
        let mut perms = metadata.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(path, perms).map_err(|e| {
            make_err!(
                nativelink_error::Code::Internal,
                "Failed to set permissions for {}: {e}",
                path.display()
            )
        })?;
    }

    Ok(())
}

/// Sets a directory tree to read-only and calculates total size in one pass.
///
/// Uses `spawn_blocking` with synchronous `std::fs` for performance.
/// Combines two walks into one to halve I/O for large trees.
pub async fn set_readonly_and_calculate_size(dir: &Path) -> Result<u64, Error> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || set_readonly_and_size_sync(&dir))
        .await
        .map_err(|e| make_err!(nativelink_error::Code::Internal, "spawn_blocking join error: {e}"))?
}

fn set_readonly_and_size_sync(path: &Path) -> Result<u64, Error> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Failed to get metadata for {}: {e}",
            path.display()
        )
    })?;

    if metadata.is_symlink() {
        return Ok(0);
    }

    if metadata.is_dir() {
        let mut total = 0u64;
        for entry in std::fs::read_dir(path).map_err(|e| {
            make_err!(
                nativelink_error::Code::Internal,
                "Failed to read directory {}: {e}",
                path.display()
            )
        })? {
            let entry = entry.map_err(|e| {
                make_err!(
                    nativelink_error::Code::Internal,
                    "Failed to read entry in {}: {e}",
                    path.display()
                )
            })?;
            total += set_readonly_and_size_sync(&entry.path())?;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = metadata.permissions();
            perms.set_mode(0o555);
            std::fs::set_permissions(path, perms).map_err(|e| {
                make_err!(
                    nativelink_error::Code::Internal,
                    "Failed to set permissions for {}: {e}",
                    path.display()
                )
            })?;
        }

        #[cfg(windows)]
        {
            let mut perms = metadata.permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(path, perms).map_err(|e| {
                make_err!(
                    nativelink_error::Code::Internal,
                    "Failed to set permissions for {}: {e}",
                    path.display()
                )
            })?;
        }

        Ok(total)
    } else if metadata.is_file() {
        let size = metadata.len();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let current_mode = metadata.permissions().mode() & 0o777;
            if current_mode != 0o555 {
                let mut perms = metadata.permissions();
                perms.set_mode(0o555);
                std::fs::set_permissions(path, perms).map_err(|e| {
                    make_err!(
                        nativelink_error::Code::Internal,
                        "Failed to set permissions for {}: {e}",
                        path.display()
                    )
                })?;
            }
        }

        #[cfg(windows)]
        {
            let mut perms = metadata.permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(path, perms).map_err(|e| {
                make_err!(
                    nativelink_error::Code::Internal,
                    "Failed to set permissions for {}: {e}",
                    path.display()
                )
            })?;
        }

        Ok(size)
    } else {
        Ok(0)
    }
}

/// Calculates the total size of a directory tree in bytes.
///
/// Uses `spawn_blocking` with synchronous `std::fs` for performance.
pub async fn calculate_directory_size(dir: &Path) -> Result<u64, Error> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || calculate_size_sync(&dir))
        .await
        .map_err(|e| make_err!(nativelink_error::Code::Internal, "spawn_blocking join error: {e}"))?
}

fn calculate_size_sync(path: &Path) -> Result<u64, Error> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Failed to get metadata for {}: {e}",
            path.display()
        )
    })?;

    if metadata.is_symlink() {
        return Ok(0);
    }

    if metadata.is_file() {
        return Ok(metadata.len());
    }

    if !metadata.is_dir() {
        return Ok(0);
    }

    let mut total = 0u64;
    for entry in std::fs::read_dir(path).map_err(|e| {
        make_err!(
            nativelink_error::Code::Internal,
            "Failed to read directory {}: {e}",
            path.display()
        )
    })? {
        let entry = entry.map_err(|e| {
            make_err!(
                nativelink_error::Code::Internal,
                "Failed to read entry in {}: {e}",
                path.display()
            )
        })?;
        total += calculate_size_sync(&entry.path())?;
    }

    Ok(total)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    use nativelink_error::ResultExt;
    use nativelink_macro::nativelink_test;
    use tempfile::TempDir;
    use tokio::fs;

    use super::*;

    async fn create_test_directory() -> Result<(TempDir, PathBuf), Error> {
        let temp_dir = TempDir::new().err_tip(|| "Failed to create temp directory")?;
        let test_dir = temp_dir.path().join("test_src");

        std::fs::create_dir(&test_dir).err_tip(|| "create test_src")?;

        let file1 = test_dir.join("file1.txt");
        let mut f = std::fs::File::create(&file1).err_tip(|| "create file1")?;
        f.write_all(b"Hello, World!").err_tip(|| "write file1")?;
        drop(f);

        let subdir = test_dir.join("subdir");
        std::fs::create_dir(&subdir).err_tip(|| "create subdir")?;

        let file2 = subdir.join("file2.txt");
        let mut f = std::fs::File::create(&file2).err_tip(|| "create file2")?;
        f.write_all(b"Nested file").err_tip(|| "write file2")?;
        drop(f);

        Ok((temp_dir, test_dir))
    }

    #[nativelink_test("crate")]
    async fn test_hardlink_directory_tree() -> Result<(), Error> {
        let (temp_dir, src_dir) = create_test_directory().await?;
        let dst_dir = temp_dir.path().join("test_dst");

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

    #[nativelink_test("crate")]
    async fn test_set_readonly_recursive() -> Result<(), Error> {
        let (_temp_dir, test_dir) = create_test_directory().await?;

        set_readonly_recursive(&test_dir).await?;

        // Verify files are read-only
        let metadata = fs::metadata(test_dir.join("file1.txt")).await?;
        assert!(metadata.permissions().readonly());

        let metadata = fs::metadata(test_dir.join("subdir/file2.txt")).await?;
        assert!(metadata.permissions().readonly());

        Ok(())
    }

    #[nativelink_test("crate")]
    async fn test_calculate_directory_size() -> Result<(), Error> {
        let (_temp_dir, test_dir) = create_test_directory().await?;

        let size = calculate_directory_size(&test_dir).await?;

        // "Hello, World!" = 13 bytes
        // "Nested file" = 11 bytes
        // Total = 24 bytes
        assert_eq!(size, 24);

        Ok(())
    }

    #[nativelink_test("crate")]
    async fn test_hardlink_nonexistent_source() {
        let temp_dir = TempDir::new().unwrap();
        let src = temp_dir.path().join("nonexistent");
        let dst = temp_dir.path().join("dest");

        let result = hardlink_directory_tree(&src, &dst).await;
        assert!(result.is_err());
    }

    #[nativelink_test("crate")]
    async fn test_hardlink_into_existing_destination() -> Result<(), Error> {
        let (temp_dir, src_dir) = create_test_directory().await?;
        let dst_dir = temp_dir.path().join("existing");

        // Pre-create the destination directory (simulates work_directory already existing)
        fs::create_dir(&dst_dir).await?;

        // Should succeed — hardlink contents into existing directory
        hardlink_directory_tree(&src_dir, &dst_dir).await?;

        // Verify structure
        assert!(dst_dir.join("file1.txt").exists());
        assert!(dst_dir.join("subdir").is_dir());
        assert!(dst_dir.join("subdir/file2.txt").exists());

        // Verify contents
        let content1 = fs::read_to_string(dst_dir.join("file1.txt")).await?;
        assert_eq!(content1, "Hello, World!");

        Ok(())
    }
}
