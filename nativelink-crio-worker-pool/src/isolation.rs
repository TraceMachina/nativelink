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

//! Isolation mechanisms for warm worker pools to prevent state leakage between jobs.
//!
//! This module provides Copy-on-Write (COW) isolation using OverlayFS, ensuring that each
//! job executes in an isolated filesystem environment while maintaining the performance
//! benefits of pre-warmed worker containers.

use std::path::{Path, PathBuf};

use nativelink_error::{Error, ResultExt};
use tokio::fs;
use tracing::{debug, warn};

/// Represents an OverlayFS mount configuration for job isolation.
#[derive(Debug, Clone)]
pub struct OverlayFsMount {
    /// Lower directory (read-only template)
    pub lower: PathBuf,
    /// Upper directory (read-write layer for this job)
    pub upper: PathBuf,
    /// Work directory (OverlayFS metadata)
    pub work: PathBuf,
    /// Merged directory (unified view presented to the job)
    pub merged: PathBuf,
}

impl OverlayFsMount {
    /// Creates a new OverlayFS mount configuration for a job.
    ///
    /// # Arguments
    /// * `template_path` - Path to the warm template container filesystem (lower layer)
    /// * `job_workspace_root` - Root directory where job-specific directories will be created
    /// * `job_id` - Unique identifier for this job
    pub fn new(template_path: &Path, job_workspace_root: &Path, job_id: &str) -> Self {
        let job_dir = job_workspace_root.join(job_id);

        Self {
            lower: template_path.to_path_buf(),
            upper: job_dir.join("upper"),
            work: job_dir.join("work"),
            merged: job_dir.join("merged"),
        }
    }

    /// Creates the directory structure required for OverlayFS.
    pub async fn create_directories(&self) -> Result<(), Error> {
        fs::create_dir_all(&self.upper)
            .await
            .err_tip(|| format!("Failed to create upper directory: {:?}", self.upper))?;

        fs::create_dir_all(&self.work)
            .await
            .err_tip(|| format!("Failed to create work directory: {:?}", self.work))?;

        fs::create_dir_all(&self.merged)
            .await
            .err_tip(|| format!("Failed to create merged directory: {:?}", self.merged))?;

        debug!(
            lower = ?self.lower,
            upper = ?self.upper,
            work = ?self.work,
            merged = ?self.merged,
            "Created OverlayFS directory structure"
        );

        Ok(())
    }

    /// Gets the OverlayFS mount options string for use with mount(2).
    ///
    /// Returns a string like: "lowerdir=/template,upperdir=/job/upper,workdir=/job/work"
    pub fn get_mount_options(&self) -> String {
        format!(
            "lowerdir={},upperdir={},workdir={}",
            self.lower.display(),
            self.upper.display(),
            self.work.display()
        )
    }

    /// Cleans up the job-specific directories after job completion.
    ///
    /// This removes the upper and work directories, leaving the template (lower) intact.
    pub async fn cleanup(&self) -> Result<(), Error> {
        // Remove upper directory
        if self.upper.exists() {
            if let Err(e) = fs::remove_dir_all(&self.upper).await {
                warn!(
                    path = ?self.upper,
                    error = ?e,
                    "Failed to remove upper directory during cleanup"
                );
            }
        }

        // Remove work directory
        if self.work.exists() {
            if let Err(e) = fs::remove_dir_all(&self.work).await {
                warn!(
                    path = ?self.work,
                    error = ?e,
                    "Failed to remove work directory during cleanup"
                );
            }
        }

        // Remove merged directory
        if self.merged.exists() {
            if let Err(e) = fs::remove_dir_all(&self.merged).await {
                warn!(
                    path = ?self.merged,
                    error = ?e,
                    "Failed to remove merged directory during cleanup"
                );
            }
        }

        // Remove parent job directory
        if let Some(parent) = self.upper.parent() {
            if parent.exists() {
                if let Err(e) = fs::remove_dir_all(parent).await {
                    warn!(
                        path = ?parent,
                        error = ?e,
                        "Failed to remove job workspace directory"
                    );
                }
            }
        }

        debug!(job_workspace = ?self.upper.parent(), "Cleaned up job workspace");

        Ok(())
    }
}

/// Snapshots a container's filesystem to create a template for COW cloning.
///
/// # Arguments
/// * `container_root` - Root filesystem path of the warm container
/// * `template_path` - Destination path where the template snapshot will be stored
///
/// # Returns
/// Ok(()) if snapshot was created successfully
pub async fn snapshot_container_filesystem(
    container_root: &Path,
    template_path: &Path,
) -> Result<(), Error> {
    // Create template directory if it doesn't exist
    fs::create_dir_all(template_path)
        .await
        .err_tip(|| format!("Failed to create template directory: {:?}", template_path))?;

    // For now, we'll use a simple directory copy
    // In production, this could be optimized with:
    // 1. Hardlinks (like directory_cache.rs)
    // 2. Filesystem snapshots (btrfs, zfs)
    // 3. Container image exports

    debug!(
        source = ?container_root,
        dest = ?template_path,
        "Snapshotting container filesystem for template"
    );

    // Use tar or similar for atomic snapshot
    // For MVP, we'll assume the container filesystem is already accessible
    // and CRI-O provides it via container inspect

    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn test_overlayfs_mount_creation() -> Result<(), Error> {
        let temp_dir = TempDir::new().err_tip(|| "Failed to create temp directory")?;
        let template_path = temp_dir.path().join("template");
        let job_workspace = temp_dir.path().join("jobs");

        fs::create_dir_all(&template_path).await?;

        let mount = OverlayFsMount::new(&template_path, &job_workspace, "test-job-123");

        // Create directories
        mount.create_directories().await?;

        // Verify directories exist
        assert!(mount.upper.exists());
        assert!(mount.work.exists());
        assert!(mount.merged.exists());

        // Verify mount options format
        let options = mount.get_mount_options();
        assert!(options.contains("lowerdir="));
        assert!(options.contains("upperdir="));
        assert!(options.contains("workdir="));

        Ok(())
    }

    #[tokio::test]
    async fn test_overlayfs_cleanup() -> Result<(), Error> {
        let temp_dir = TempDir::new().err_tip(|| "Failed to create temp directory")?;
        let template_path = temp_dir.path().join("template");
        let job_workspace = temp_dir.path().join("jobs");

        fs::create_dir_all(&template_path).await?;

        let mount = OverlayFsMount::new(&template_path, &job_workspace, "test-job-456");
        mount.create_directories().await?;

        // Verify directories exist before cleanup
        assert!(mount.upper.exists());
        assert!(mount.work.exists());

        // Cleanup
        mount.cleanup().await?;

        // Verify directories are removed
        assert!(!mount.upper.exists());
        assert!(!mount.work.exists());
        assert!(!mount.merged.exists());

        // Template should remain
        assert!(template_path.exists());

        Ok(())
    }

    #[tokio::test]
    async fn test_isolation_workflow() -> Result<(), Error> {
        let temp_dir = TempDir::new().err_tip(|| "Failed to create temp directory")?;
        let template_path = temp_dir.path().join("template");
        let job_workspace = temp_dir.path().join("jobs");

        // Simulate template creation
        fs::create_dir_all(&template_path).await?;
        fs::write(template_path.join("template_file.txt"), b"template data").await?;

        // Clone for job1
        let mount1 = OverlayFsMount::new(&template_path, &job_workspace, "job1");
        mount1.create_directories().await?;

        // Clone for job2
        let mount2 = OverlayFsMount::new(&template_path, &job_workspace, "job2");
        mount2.create_directories().await?;

        // Verify both jobs have isolated directories
        assert!(mount1.upper.exists());
        assert!(mount2.upper.exists());
        assert_ne!(mount1.upper, mount2.upper);

        // Cleanup job1
        mount1.cleanup().await?;
        assert!(!mount1.upper.exists());

        // Verify job2 is unaffected and template remains
        assert!(mount2.upper.exists());
        assert!(template_path.exists());

        // Cleanup job2
        mount2.cleanup().await?;
        assert!(!mount2.upper.exists());
        assert!(template_path.exists());

        Ok(())
    }
}
