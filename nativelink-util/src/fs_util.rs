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

use core::future::Future;
use core::pin::Pin;
use std::fs::Metadata;
use std::path::{Path, PathBuf};

use nativelink_error::{Code, Error, ResultExt, error_if, make_err};
use tokio::fs;

/// Hardlinks an entire directory tree from source to destination.
/// This is much faster than copying for large directory structures.
///
/// # Arguments
/// * `src_dir` - Source directory path (must exist)
/// * `dst_dir` - Destination directory path (will be created)
///
/// # Returns
/// * `Ok(())` on success
/// * `Err` if hardlinking fails (e.g., cross-filesystem, unsupported filesystem)
///
/// # Platform Support
/// - Linux: Full support via `fs::hard_link`
/// - macOS: Full support via `fs::hard_link`
/// - Windows: Requires NTFS filesystem and appropriate permissions
///
/// # Errors
/// - Source directory doesn't exist
/// - Destination already exists
/// - Cross-filesystem hardlinking attempted
/// - Filesystem doesn't support hardlinks
/// - Permission denied
pub async fn hardlink_directory_tree(src_dir: &Path, dst_dir: &Path) -> Result<(), Error> {
    error_if!(
        !src_dir.exists(),
        "Source directory does not exist: {}",
        src_dir.display()
    );

    error_if!(
        dst_dir.exists(),
        "Destination directory already exists: {}",
        dst_dir.display()
    );

    // Create the root destination directory
    fs::create_dir_all(dst_dir).await.err_tip(|| {
        format!(
            "Failed to create destination directory: {}",
            dst_dir.display()
        )
    })?;

    // Recursively hardlink the directory tree
    hardlink_directory_tree_recursive(src_dir, dst_dir).await
}

/// Internal recursive function to hardlink directory contents
fn hardlink_directory_tree_recursive<'a>(
    src: &'a Path,
    dst: &'a Path,
) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
    Box::pin(async move {
        let mut entries = fs::read_dir(src)
            .await
            .err_tip(|| format!("Failed to read directory: {}", src.display()))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .err_tip(|| format!("Failed to get next entry in: {}", src.display()))?
        {
            let entry_path = entry.path();
            let file_name = entry.file_name().into_string().map_err(|os_str| {
                make_err!(
                    Code::InvalidArgument,
                    "Invalid UTF-8 in filename: {:?}",
                    os_str
                )
            })?;

            let dst_path = dst.join(&file_name);
            let metadata = entry
                .metadata()
                .await
                .err_tip(|| format!("Failed to get metadata for: {}", entry_path.display()))?;

            if metadata.is_dir() {
                // Create subdirectory and recurse
                fs::create_dir(&dst_path)
                    .await
                    .err_tip(|| format!("Failed to create directory: {}", dst_path.display()))?;

                hardlink_directory_tree_recursive(&entry_path, &dst_path).await?;
            } else if metadata.is_file() {
                // Hardlink the file
                fs::hard_link(&entry_path, &dst_path)
                    .await
                    .err_tip(|| {
                        format!(
                            "Failed to hardlink {} to {}. This may occur if the source and destination are on different filesystems",
                            entry_path.display(),
                            dst_path.display()
                        )
                    })?;
            } else if metadata.is_symlink() {
                // Read the symlink target and create a new symlink
                let target = fs::read_link(&entry_path)
                    .await
                    .err_tip(|| format!("Failed to read symlink: {}", entry_path.display()))?;

                #[cfg(unix)]
                fs::symlink(&target, &dst_path)
                    .await
                    .err_tip(|| format!("Failed to create symlink: {}", dst_path.display()))?;

                #[cfg(windows)]
                {
                    if target.is_dir() {
                        fs::symlink_dir(&target, &dst_path).await.err_tip(|| {
                            format!("Failed to create directory symlink: {}", dst_path.display())
                        })?;
                    } else {
                        fs::symlink_file(&target, &dst_path).await.err_tip(|| {
                            format!("Failed to create file symlink: {}", dst_path.display())
                        })?;
                    }
                }
            }
        }

        Ok(())
    })
}

/// Sets a directory tree to read-only recursively.
/// This prevents actions from modifying cached directories.
///
/// # Arguments
/// * `dir` - Directory to make read-only
///
/// # Platform Notes
/// - Unix: Sets permissions to 0o555 (r-xr-xr-x)
/// - Windows: Sets `FILE_ATTRIBUTE_READONLY`
pub async fn set_readonly_recursive(dir: &Path) -> Result<(), Error> {
    error_if!(!dir.exists(), "Directory does not exist: {}", dir.display());

    set_perms_recursive_impl(dir.to_path_buf(), set_readonly_one_path).await
}

/// Sets a directory tree to read-write for the current user recursively.
/// This is done so we can delete directories we're evicting.
///
/// # Arguments
/// * `dir` - Directory to make read-write
///
/// # Platform Notes
/// - Unix: Sets permissions to 0o755 (rwxr-xr-x)
/// - Windows: Clears `FILE_ATTRIBUTE_READONLY`
pub async fn set_readwrite_recursive(dir: &Path) -> Result<(), Error> {
    error_if!(!dir.exists(), "Directory does not exist: {}", dir.display());

    set_perms_recursive_impl(dir.to_path_buf(), set_readwrite_one_path).await
}

/// Sets only the **directories** in a tree to writable for the current user,
/// leaving files untouched. This is the safe variant for cleanup paths that
/// need to delete a tree containing CAS-hardlinked files.
///
/// On unix, write permission on the parent directory is sufficient to unlink
/// files inside it — the files' own modes are irrelevant for unlinking. Chmoding
/// a CAS-hardlinked file would silently mutate the shared inode's permissions
/// for every other in-flight action that has hardlinked the same blob, leading
/// to EACCES on exec or EPERM on open in unrelated actions.
///
/// # Arguments
/// * `dir` - Directory whose directories should be made writable
///
/// # Platform Notes
/// - Unix: Sets directory permissions to 0o755 (rwxr-xr-x); files are NOT touched.
/// - Windows: Clears `FILE_ATTRIBUTE_READONLY` on directories only; files are NOT touched.
pub async fn set_dir_writable_recursive(dir: &Path) -> Result<(), Error> {
    error_if!(!dir.exists(), "Directory does not exist: {}", dir.display());

    set_perms_recursive_impl(dir.to_path_buf(), set_dir_writable_one_path).await
}

fn set_readonly_one_path(
    path: PathBuf,
    metadata: Metadata,
) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
    Box::pin(async move {
        // Set the file/directory to read-only
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = metadata.permissions();

            // If it's a directory, set to r-xr-xr-x (555)
            // If it's a file, set to r--r--r-- (444)
            let mode = if metadata.is_dir() { 0o555 } else { 0o444 };
            perms.set_mode(mode);

            fs::set_permissions(&path, perms)
                .await
                .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
        }

        #[cfg(windows)]
        {
            let mut perms = metadata.permissions();
            perms.set_readonly(true);

            fs::set_permissions(&path, perms)
                .await
                .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
        }

        Ok(())
    })
}

fn set_readwrite_one_path(
    path: PathBuf,
    metadata: Metadata,
) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
    Box::pin(async move {
        // Set the file/directory to read-write for the current user
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = metadata.permissions();

            // If it's a directory, set to rwxr-xr-x (755)
            // If it's a file, set to rw-r--r-- (644)
            let mode = if metadata.is_dir() { 0o755 } else { 0o644 };
            perms.set_mode(mode);

            fs::set_permissions(&path, perms)
                .await
                .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
        }

        #[cfg(windows)]
        {
            let mut perms = metadata.permissions();
            perms.set_readonly(false);

            fs::set_permissions(&path, perms)
                .await
                .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
        }

        Ok(())
    })
}

fn set_dir_writable_one_path(
    path: PathBuf,
    metadata: Metadata,
) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
    Box::pin(async move {
        // Files are intentionally skipped here. They may be hardlinked into
        // the CAS (FilesystemStore); chmoding them would corrupt the shared
        // inode's mode for every other in-flight action.
        if !metadata.is_dir() {
            return Ok(());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);

            fs::set_permissions(&path, perms)
                .await
                .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
        }

        #[cfg(windows)]
        {
            let mut perms = metadata.permissions();
            perms.set_readonly(false);

            fs::set_permissions(&path, perms)
                .await
                .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
        }

        Ok(())
    })
}

fn set_perms_recursive_impl<'a, F>(
    path: PathBuf,
    perms_fn: F,
) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>
where
    F: Fn(PathBuf, Metadata) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>
        + Send
        + Copy
        + 'a,
{
    Box::pin(async move {
        let metadata = fs::metadata(&path)
            .await
            .err_tip(|| format!("Failed to get metadata for: {}", path.display()))?;

        if metadata.is_dir() {
            let mut entries = fs::read_dir(&path)
                .await
                .err_tip(|| format!("Failed to read directory: {}", path.display()))?;

            while let Some(entry) = entries
                .next_entry()
                .await
                .err_tip(|| format!("Failed to get next entry in: {}", path.display()))?
            {
                set_perms_recursive_impl(entry.path(), perms_fn).await?;
            }
        }
        perms_fn(path, metadata).await
    })
}

/// Calculates the total size of a directory tree in bytes.
/// Used for cache size tracking and LRU eviction.
///
/// # Arguments
/// * `dir` - Directory to calculate size for
///
/// # Returns
/// Total size in bytes, or Error if directory cannot be read
pub async fn calculate_directory_size(dir: &Path) -> Result<u64, Error> {
    error_if!(!dir.exists(), "Directory does not exist: {}", dir.display());

    calculate_directory_size_impl(dir).await
}

fn calculate_directory_size_impl<'a>(
    path: &'a Path,
) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + 'a>> {
    Box::pin(async move {
        let metadata = fs::metadata(path)
            .await
            .err_tip(|| format!("Failed to get metadata for: {}", path.display()))?;

        if metadata.is_file() {
            return Ok(metadata.len());
        }

        if !metadata.is_dir() {
            return Ok(0);
        }

        let mut total_size = 0u64;
        let mut entries = fs::read_dir(path)
            .await
            .err_tip(|| format!("Failed to read directory: {}", path.display()))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .err_tip(|| format!("Failed to get next entry in: {}", path.display()))?
        {
            total_size += calculate_directory_size_impl(&entry.path()).await?;
        }

        Ok(total_size)
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use nativelink_macro::nativelink_test;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;

    use super::*;

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
    async fn test_hardlink_existing_destination() -> Result<(), Error> {
        let (temp_dir, src_dir) = create_test_directory().await?;
        let dst_dir = temp_dir.path().join("existing");

        fs::create_dir(&dst_dir).await?;

        let result = hardlink_directory_tree(&src_dir, &dst_dir).await;
        assert!(result.is_err());

        Ok(())
    }
}
