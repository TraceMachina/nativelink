// Copyright 2026 The NativeLink Authors. All rights reserved.
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

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::{Command, Directory};
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_util::common::DigestInfo;
use parking_lot::Mutex;

use crate::directory_cache::{DigestLease, DirectoryCache, PreparedDirectory};

/// Selected immutable input subtrees and their cache pins for one action.
/// Pins remain held through cleanup, including after cancelled preparation.
#[derive(Debug)]
pub struct InputMounts {
    cache: Arc<DirectoryCache>,
    targets: HashSet<PathBuf>,
    prepared: Mutex<Vec<(PathBuf, PreparedDirectory)>>,
}

/// Validates that mount paths are canonical, nonempty, and non-overlapping.
/// Paths are relative to the action input root. Mounting the whole root would
/// prevent the action from creating outputs.
pub fn validate_paths(paths: &[String]) -> Result<(), Error> {
    for (i, path) in paths.iter().enumerate() {
        if !is_canonical_relative(path)
            || paths[..i]
                .iter()
                .any(|other| paths_overlap(Path::new(path), Path::new(other)))
        {
            return Err(make_err!(
                Code::InvalidArgument,
                "Read-only input mount paths must be nonempty, canonical, non-overlapping relative paths: {path:?}"
            ));
        }
    }
    Ok(())
}

/// Checks only the ancestors of selected paths, without walking their contents.
/// Actions without a matching directory should keep using the ordinary cache.
/// `paths` must have passed `validate_paths`.
pub async fn has_mountable_inputs(
    cas_store: &FastSlowStore,
    root_digest: DigestInfo,
    paths: &[String],
    lease: Option<&dyn DigestLease>,
) -> Result<bool, Error> {
    'target: for path in paths {
        let mut digest = root_digest;
        for component in path.split('/') {
            if let Some(lease) = lease {
                lease.acquire(&digest);
            }
            let directory = get_and_decode_digest::<Directory>(cas_store, digest.into()).await?;
            let Some(child) = directory
                .directories
                .iter()
                .find(|child| child.name == component)
            else {
                continue 'target;
            };
            digest = child
                .digest
                .as_ref()
                .err_tip(|| "Missing input directory digest")?
                .try_into()?;
        }
        return Ok(true);
    }
    Ok(false)
}

fn is_canonical_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0'])
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

// Outputs are relative to the command's working directory and may use `..`
// (for example Chromium's ../clang-crashreports). Normalize lexically within
// the input root before checking overlap. Symlink aliases remain a workload
// constraint; resolving them would require walking the input tree on every hit.
fn resolve_output(cwd: &str, output: &str) -> Option<PathBuf> {
    if output.contains(['\\', '\0']) {
        return None;
    }
    let mut path = PathBuf::from(cwd);
    for component in Path::new(output).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !path.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(path)
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

impl InputMounts {
    /// Creates mount state for a compatible command, or returns `None` to use
    /// ordinary materialization. Declared outputs must not overlap a selected
    /// subtree: the output collector cannot see the child's private mounts.
    pub fn for_command(
        cache: Arc<DirectoryCache>,
        paths: &[String],
        root: &Path,
        command: &Command,
    ) -> Option<Arc<Self>> {
        let cwd = &command.working_directory;
        if !cwd.is_empty() && !is_canonical_relative(cwd) {
            return None;
        }
        if paths.iter().any(|path| Path::new(cwd).starts_with(path)) {
            return None;
        }
        for output in command
            .output_paths
            .iter()
            .chain(&command.output_files)
            .chain(&command.output_directories)
        {
            // Empty output paths designate the whole working directory.
            let output = resolve_output(cwd, output)?;
            if paths
                .iter()
                .any(|path| paths_overlap(&output, Path::new(path)))
            {
                return None;
            }
        }
        Some(Arc::new(Self {
            cache,
            targets: paths.iter().map(|path| root.join(path)).collect(),
            prepared: Mutex::new(Vec::new()),
        }))
    }

    /// Prepares and pins a selected subtree before the input walk descends into it.
    /// Returns `true` when the caller should leave an empty mount point instead
    /// of materializing the subtree, or `false` when the path is not selected.
    pub async fn prepare(
        &self,
        path: &Path,
        digest: DigestInfo,
        lease: Option<&dyn DigestLease>,
    ) -> Result<bool, Error> {
        if !self.targets.contains(path) {
            return Ok(false);
        }
        let directory = self.cache.prepare_for_mount(digest, lease).await?;
        self.prepared.lock().push((path.to_owned(), directory));
        Ok(true)
    }

    /// Allocates mount paths and reads mount flags before entering pre-exec.
    #[cfg(target_os = "linux")]
    pub fn bind_mounts(&self) -> Result<Vec<crate::namespace_utils::ReadOnlyBindMount>, Error> {
        self.prepared
            .lock()
            .iter()
            .map(|(target, directory)| {
                crate::namespace_utils::ReadOnlyBindMount::new(directory.path(), target)
                    .map_err(Error::from)
            })
            .collect()
    }
}
