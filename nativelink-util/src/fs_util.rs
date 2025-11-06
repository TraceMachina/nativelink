// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::future::Future;
use core::pin::Pin;
use std::path::Path;

use nativelink_error::{Code, Error, ResultExt, error_if, make_err};
use tokio::fs;

/// Hardlinks an entire directory tree from source to destination.
/// This is much faster than copying for large directory structures.
///
/// # Arguments
/// * `src_dir` - Source directory path (must exist)
/// * `dst_dir` - Destination directory path (will be created if it doesn't exist)
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
/// - Cross-filesystem hardlinking attempted
/// - Filesystem doesn't support hardlinks
/// - Permission denied
pub async fn hardlink_directory_tree(src_dir: &Path, dst_dir: &Path) -> Result<(), Error> {
    error_if!(
        !src_dir.exists(),
        "Source directory does not exist: {:?}",
        src_dir
    );

    // Only create the root destination directory if it doesn't exist
    // This allows the function to be called on already-created directories
    if !dst_dir.exists() {
        fs::create_dir_all(dst_dir)
            .await
            .err_tip(|| format!("Failed to create destination directory: {dst_dir:?}"))?;
    }

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
            .err_tip(|| format!("Failed to read directory: {src:?}"))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .err_tip(|| format!("Failed to get next entry in: {src:?}"))?
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
                .err_tip(|| format!("Failed to get metadata for: {entry_path:?}"))?;

            if metadata.is_dir() {
                // Create subdirectory and recurse
                fs::create_dir(&dst_path)
                    .await
                    .err_tip(|| format!("Failed to create directory: {dst_path:?}"))?;

                hardlink_directory_tree_recursive(&entry_path, &dst_path).await?;
            } else if metadata.is_file() {
                // Hardlink the file
                fs::hard_link(&entry_path, &dst_path)
                    .await
                    .err_tip(|| {
                        format!(
                            "Failed to hardlink {entry_path:?} to {dst_path:?}. This may occur if the source and destination are on different filesystems"
                        )
                    })?;
            } else if metadata.is_symlink() {
                // Read the symlink target and create a new symlink
                let target = fs::read_link(&entry_path)
                    .await
                    .err_tip(|| format!("Failed to read symlink: {entry_path:?}"))?;

                #[cfg(unix)]
                fs::symlink(&target, &dst_path)
                    .await
                    .err_tip(|| format!("Failed to create symlink: {dst_path:?}"))?;

                #[cfg(windows)]
                {
                    if target.is_dir() {
                        fs::symlink_dir(&target, &dst_path).await.err_tip(|| {
                            format!("Failed to create directory symlink: {:?}", dst_path)
                        })?;
                    } else {
                        fs::symlink_file(&target, &dst_path)
                            .await
                            .err_tip(|| format!("Failed to create file symlink: {:?}", dst_path))?;
                    }
                }
            }
        }

        Ok(())
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
    error_if!(!dir.exists(), "Directory does not exist: {:?}", dir);

    calculate_directory_size_impl(dir).await
}

fn calculate_directory_size_impl<'a>(
    path: &'a Path,
) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + 'a>> {
    Box::pin(async move {
        let metadata = fs::metadata(path)
            .await
            .err_tip(|| format!("Failed to get metadata for: {path:?}"))?;

        if metadata.is_file() {
            return Ok(metadata.len());
        }

        if !metadata.is_dir() {
            return Ok(0);
        }

        let mut total_size = 0u64;
        let mut entries = fs::read_dir(path)
            .await
            .err_tip(|| format!("Failed to read directory: {path:?}"))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .err_tip(|| format!("Failed to get next entry in: {path:?}"))?
        {
            total_size += calculate_directory_size_impl(&entry.path()).await?;
        }

        Ok(total_size)
    })
}
