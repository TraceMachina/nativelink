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
#[cfg(target_os = "macos")]
use tracing::debug;

/// Which kernel mechanism actually materialized the destination tree.
/// Returned by [`hardlink_directory_tree`] so callers can record per-hit
/// telemetry and detect when the fast path silently degrades (e.g., a
/// cross-volume cache layout that forces clonefile to fall through to
/// per-file hardlinks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloneMethod {
    /// APFS `clonefile(2)` succeeded — O(1) regardless of tree size.
    /// macOS only.
    Clonefile,
    /// Per-file `fs::hard_link` walk — O(N) in file count.
    /// Used on Linux/Windows always, and on macOS when clonefile fell through.
    Hardlink,
}

/// Materializes an entire directory tree from source to destination using the
/// fastest method the host filesystem supports.
///
/// # Arguments
/// * `src_dir` - Source directory path (must exist)
/// * `dst_dir` - Destination directory path (must NOT exist; parent will be created)
///
/// # Returns
/// * `Ok(CloneMethod)` indicating which kernel mechanism was used
/// * `Err` if materialization fails (e.g., cross-filesystem, unsupported filesystem)
///
/// # Platform Support
/// - macOS: Tries APFS `clonefile(2)` first (O(1), copy-on-write). On failure
///   (e.g., cross-volume EXDEV, or any unexpected errno) falls back to per-file
///   `fs::hard_link`. `clonefile(2)` copies the source's modes verbatim, so
///   the destination's directory/file modes mirror the source. For a directory
///   cache entry locked down by [`set_readonly_recursive`], that means
///   directories are writable (0o755) and files are read-only (0o555): the
///   worker can create the action's declared outputs at any nested path, but
///   the hardlinked input files stay immutable. This matches the hermeticity
///   contract enforced by Bazel's local sandbox and the REAPI
///   `Action.output_files` semantics: actions can only write to declared
///   outputs, never mutate inputs. The COW semantics of `clonefile(2)` mean
///   any writes the worker does make to the destination do not affect the
///   source. The destination root is additionally chmod'd to 0o755 as a
///   defensive guarantee for callers that did not pre-mark the source.
/// - Linux: Per-file `fs::hard_link` (directory hardlinks are not supported on
///   ext4/btrfs without root). Directories at the destination are created
///   fresh by this walk, so they are writable regardless of the source's
///   directory modes; files are hardlinked and keep the source inode's mode.
///   Always returns `CloneMethod::Hardlink`.
/// - Windows: Per-file `fs::hard_link` (requires NTFS). Always returns
///   `CloneMethod::Hardlink`.
///
/// # Errors
/// - Source directory doesn't exist
/// - Destination already exists
/// - Cross-filesystem materialization attempted and fallback also fails
/// - Filesystem doesn't support hardlinks (Linux/Windows fallback)
/// - Permission denied
pub async fn hardlink_directory_tree(src_dir: &Path, dst_dir: &Path) -> Result<CloneMethod, Error> {
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

    #[cfg(target_os = "macos")]
    {
        // clonefile(2) requires dst's parent to exist but dst itself must NOT
        // exist. Make sure the parent is present without creating dst. The
        // non-macOS fallback path below creates dst (and any missing parents)
        // itself via `fs::create_dir_all(dst_dir)`, so this pre-step is only
        // needed for the clonefile case.
        if let Some(parent) = dst_dir.parent() {
            fs::create_dir_all(parent).await.err_tip(|| {
                format!(
                    "Failed to create parent of destination: {}",
                    parent.display()
                )
            })?;
        }

        match try_clonefile(src_dir, dst_dir).await {
            Ok(()) => {
                // `clonefile(2)` copies the source's modes verbatim. A
                // directory cache entry locked down by
                // `set_readonly_recursive` already has writable directories
                // (0o755) and read-only files (0o555), so the clone is
                // immediately usable: the worker can create declared outputs
                // at any nested path and the hardlinked inputs stay
                // immutable. No per-directory chmod walk is needed. The root
                // is still chmod'd here as a defensive guarantee for callers
                // that pass a source whose root was not pre-marked writable.
                chmod_dir_writable(dst_dir)
                    .await
                    .err_tip(|| "Failed to chmod cloned tree root")?;
                return Ok(CloneMethod::Clonefile);
            }
            Err(e) => {
                debug!(
                    src = %src_dir.display(),
                    dst = %dst_dir.display(),
                    error = %e,
                    "clonefile failed, falling back to per-file hardlinks"
                );
                // clonefile(2) is atomic — on failure dst should not exist —
                // but be defensive in case a partial tree was left behind.
                let _cleanup = fs::remove_dir_all(dst_dir).await;
            }
        }
    }

    // Create the root destination directory
    fs::create_dir_all(dst_dir).await.err_tip(|| {
        format!(
            "Failed to create destination directory: {}",
            dst_dir.display()
        )
    })?;

    // Recursively hardlink the directory tree
    hardlink_directory_tree_recursive(src_dir, dst_dir).await?;
    Ok(CloneMethod::Hardlink)
}

/// Recursively clones a directory tree using APFS `clonefile(2)`. On success
/// the destination shares data blocks with the source via copy-on-write; the
/// operation is O(1) in tree size regardless of file count.
///
/// Returns `Err` on EXDEV (cross-volume), ENOTSUP (filesystem doesn't support
/// clones), or any other errno; callers are expected to fall back to per-file
/// hardlinks.
#[cfg(target_os = "macos")]
async fn try_clonefile(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // From <sys/clonefile.h>: don't follow symlinks at the top level. Symlinks
    // *within* the cloned tree are cloned as symlinks regardless. The `libc`
    // crate exposes `clonefile` but not this flag constant.
    const CLONE_NOFOLLOW: u32 = 0x0001;

    let src_c = CString::new(src.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "src path contains interior NUL byte",
        )
    })?;
    let dst_c = CString::new(dst.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "dst path contains interior NUL byte",
        )
    })?;

    crate::spawn_blocking!("clonefile", move || {
        // SAFETY: clonefile(2) takes two NUL-terminated C strings and a flag
        // word. Both CStrings are owned by this closure for the duration of
        // the call, so the pointers stay valid.
        let res = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), CLONE_NOFOLLOW) };
        if res == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Sets the directory `dir`'s mode to 0o755 so callers can create new
/// entries inside it. Used after `clonefile(2)` on the materialized
/// destination root as a defensive guarantee: a directory cache entry locked
/// down by [`set_readonly_recursive`] already has writable directories, so
/// for those callers this is a no-op, but it keeps `hardlink_directory_tree`
/// correct for any source whose root was not pre-marked writable. Existing
/// entries inside `dir` are intentionally left at their cloned perms — files
/// stay read-only (the hermeticity contract), directories stay writable.
#[cfg(target_os = "macos")]
async fn chmod_dir_writable(dir: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
        .await
        .err_tip(|| format!("Failed to chmod {} to 0o755", dir.display()))
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
            // `DirEntry::metadata` does NOT traverse symlinks (it has
            // `symlink_metadata`/lstat semantics), so `is_symlink()` below
            // correctly identifies symlink entries and the symlink branch
            // recreates them as symlinks rather than dereferencing them.
            let metadata = entry
                .metadata()
                .await
                .err_tip(|| format!("Failed to get metadata for: {}", entry_path.display()))?;

            if metadata.is_symlink() {
                // Recreate the symlink as a symlink. Checked BEFORE `is_dir()`
                // / `is_file()` so a symlink that resolves to a directory is
                // never treated as a real directory and recursed *through*
                // (which would dereference the link and potentially escape
                // the tree).
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
            } else if metadata.is_dir() {
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
            }
        }

        Ok(())
    })
}

/// Locks down a directory tree as an immutable cache entry: every **file** is
/// made read-only, every **directory** is left writable.
///
/// This is used by the worker's directory cache after it constructs a cache
/// entry. Files must be read-only because they are hardlinked into the CAS
/// (`FilesystemStore`) — keeping them immutable preserves the hermeticity
/// contract (actions cannot mutate inputs) and avoids mutating the shared
/// inode's mode for other in-flight actions.
///
/// Directories are deliberately left writable (0o755). Directories are *not*
/// hardlink-shared between cache entries — only file content inodes are — so a
/// writable directory mode is safe. Keeping cache-entry directories writable
/// means the materialized destination tree (an APFS `clonefile(2)` clone,
/// which copies modes verbatim, or a per-file hardlink walk, which creates
/// fresh directories) already has writable directories. Bazel actions declare
/// outputs at paths nested inside input subdirectories, so every directory in
/// the materialized tree must be writable for the worker to create those
/// outputs; doing it here, once per cache entry, removes the need for a
/// separate per-materialization recursive chmod walk.
///
/// # Arguments
/// * `dir` - Directory tree to lock down
///
/// # Platform Notes
/// - Unix: files get 0o555 (r-xr-xr-x); directories get 0o755 (rwxr-xr-x).
/// - Windows: files get `FILE_ATTRIBUTE_READONLY`; directories are left
///   writable.
///
/// Symlink entries in the tree are skipped (their own mode is not meaningful
/// and `chmod` would follow the link) - see `set_perms_recursive_impl`.
pub async fn set_readonly_recursive(dir: &Path) -> Result<(), Error> {
    error_if!(!dir.exists(), "Directory does not exist: {}", dir.display());

    set_perms_recursive_impl(dir.to_path_buf(), set_readonly_one_path).await
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
///
/// Symlink entries in the tree are skipped (their own mode is not meaningful
/// and `chmod` would follow the link) - see `set_perms_recursive_impl`.
pub async fn set_dir_writable_recursive(dir: &Path) -> Result<(), Error> {
    error_if!(!dir.exists(), "Directory does not exist: {}", dir.display());

    set_perms_recursive_impl(dir.to_path_buf(), set_dir_writable_one_path).await
}

fn set_readonly_one_path(
    path: PathBuf,
    metadata: Metadata,
) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
    Box::pin(async move {
        // Directories are left writable on purpose. They are not
        // hardlink-shared between cache entries — only file content inodes
        // are — so a writable directory mode cannot corrupt anything. Keeping
        // them writable means the materialized destination tree already
        // accepts the nested output files Bazel actions declare, with no
        // separate per-materialization chmod walk.
        if metadata.is_dir() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = metadata.permissions();
                perms.set_mode(0o755);

                fs::set_permissions(&path, perms)
                    .await
                    .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
            }

            // On Windows directories are already writable; clearing the
            // read-only attribute here would be a no-op, so leave them alone.

            return Ok(());
        }

        // Set the file to read-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = metadata.permissions();

            // Files get r-xr-xr-x (0o555): read and execute for everyone,
            // write for no one. Files use 0o555 rather than 0o444 so the
            // execute bit survives on cached executables — a stripped +x bit
            // makes an action's interpreter or wrapper script fail with
            // EACCES once the tree is materialized into a workspace. The
            // write bit stays cleared, so the hermeticity contract (inputs
            // are immutable) is unchanged.
            perms.set_mode(0o555);

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
        // Use `symlink_metadata` (lstat) rather than `metadata` (stat) so the
        // walk inspects the entry *itself*, never the target a symlink points
        // at. This matters for input trees containing symlinks - e.g.
        // `.venv/bin/python3` created by rules_python / rules_apple venv
        // tooling. With plain `stat`, a symlink to a directory reports
        // `is_dir() == true` and the walk would recurse *through* the link
        // (escaping the tree, or descending into an unrelated directory), and
        // a symlink to a file would have `chmod` applied to it - and `chmod`
        // follows symlinks, so it mutates the target. A symlink whose target
        // does not exist (a dangling link, common when a venv points outside
        // the action's input set) then fails the whole walk with ENOENT -
        // the cause of directory-cache actions falling back to the slow
        // download path.
        let metadata = fs::symlink_metadata(&path)
            .await
            .err_tip(|| format!("Failed to get metadata for: {}", path.display()))?;

        // Symlinks are skipped entirely: their own mode is not meaningful, a
        // `chmod` on the link path would follow it and touch the target, and
        // descending into a symlinked directory would walk outside the tree.
        // The symlink entry itself is left exactly as created.
        if metadata.is_symlink() {
            return Ok(());
        }

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
    #[nativelink_test("crate")]
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
    #[nativelink_test("crate")]
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
    #[nativelink_test("crate")]
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
    #[nativelink_test("crate")]
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
    #[nativelink_test("crate")]
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
    #[nativelink_test("crate")]
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
    #[nativelink_test("crate")]
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
    #[nativelink_test("crate")]
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
    #[nativelink_test("crate")]
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
    #[nativelink_test("crate")]
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
