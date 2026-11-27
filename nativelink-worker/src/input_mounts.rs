// Copyright 2026 Trace Machina, Inc. All rights reserved.
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
    selection: Selection,
    #[cfg(target_os = "linux")]
    root: PathBuf,
    prepared: Mutex<Vec<(PathBuf, PreparedDirectory)>>,
    /// Pins for the single files selected by pattern.
    prepared_files: Mutex<Vec<PreparedDirectory>>,
}

/// How an action's input directories are chosen for reuse.
#[derive(Debug)]
enum Selection {
    /// These absolute paths, all compatible with the action's command.
    Exact(HashSet<PathBuf>),
    /// Any directory or file matching a pattern, unless the command works
    /// inside it or declares an output that overlaps it. Paths other than `root` are
    /// relative to the input root.
    Patterns {
        patterns: Arc<PathPatterns>,
        root: PathBuf,
        working_directory: PathBuf,
        outputs: Vec<PathBuf>,
    },
}

/// Patterns over paths relative to the action input root. Components are
/// separated by `/`. `*` inside a component matches any run of characters;
/// a component that is exactly `**` matches any number of directories,
/// including none. Without wildcards a pattern names one path.
#[derive(Debug)]
pub struct PathPatterns {
    patterns: Vec<Vec<Segment>>,
}

#[derive(Debug)]
enum Segment {
    AnyDepth,
    Name(String),
}

/// The positions reached in each pattern after matching some path: for
/// every pattern, the indexes of the segments a child name may match next.
#[derive(Debug, Clone)]
pub struct PatternStates<'a> {
    patterns: &'a PathPatterns,
    positions: Vec<Vec<usize>>,
}

impl PathPatterns {
    /// Rejects patterns that are empty, absolute, non-canonical, that use
    /// `**` as part of a longer component, or that end in `**`: a pattern
    /// has to name the directory to reuse.
    pub fn parse(patterns: &[String]) -> Result<Self, Error> {
        let mut parsed = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let valid = is_canonical_relative(pattern)
                && pattern
                    .split('/')
                    .all(|part| part == "**" || !part.contains("**"))
                && !pattern.ends_with("**");
            if !valid {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "Read-only input patterns must be nonempty, canonical, relative paths, with `**` only as a whole component that is not the last: {pattern:?}"
                ));
            }
            parsed.push(
                pattern
                    .split('/')
                    .map(|part| match part {
                        "**" => Segment::AnyDepth,
                        name => Segment::Name(name.to_owned()),
                    })
                    .collect(),
            );
        }
        Ok(Self { patterns: parsed })
    }

    pub const fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// The states at the input root, before any component is matched.
    pub fn start(&self) -> PatternStates<'_> {
        let mut states = PatternStates {
            patterns: self,
            positions: vec![vec![0]; self.patterns.len()],
        };
        states.close();
        states
    }

    /// Whether `path`, relative to the input root, matches a pattern.
    pub fn matches(&self, path: &Path) -> bool {
        let mut states = self.start();
        for component in path.components() {
            let Component::Normal(name) = component else {
                return false;
            };
            let Some(name) = name.to_str() else {
                return false;
            };
            states = states.descend(name);
        }
        states.is_match()
    }
}

impl PatternStates<'_> {
    /// A `**` may match nothing, so its successor is reachable wherever it is.
    fn close(&mut self) {
        for (segments, positions) in self.patterns.patterns.iter().zip(&mut self.positions) {
            let mut i = 0;
            while i < positions.len() {
                let position = positions[i];
                if matches!(segments.get(position), Some(Segment::AnyDepth))
                    && !positions.contains(&(position + 1))
                {
                    positions.push(position + 1);
                }
                i += 1;
            }
        }
    }

    /// The states after one more path component named `name`.
    #[must_use]
    pub fn descend(&self, name: &str) -> Self {
        let mut next = Self {
            patterns: self.patterns,
            positions: Vec::with_capacity(self.positions.len()),
        };
        for (segments, positions) in self.patterns.patterns.iter().zip(&self.positions) {
            let mut reached = Vec::new();
            for &position in positions {
                let target = match segments.get(position) {
                    Some(Segment::AnyDepth) => position,
                    Some(Segment::Name(pattern)) if wildcard_match(pattern, name) => position + 1,
                    _ => continue,
                };
                if !reached.contains(&target) {
                    reached.push(target);
                }
            }
            next.positions.push(reached);
        }
        next.close();
        next
    }

    /// Whether the path walked so far matches a pattern in full.
    pub fn is_match(&self) -> bool {
        self.patterns
            .patterns
            .iter()
            .zip(&self.positions)
            .any(|(segments, positions)| positions.contains(&segments.len()))
    }

    /// Whether no path below the one walked so far can match any pattern.
    pub fn is_dead(&self) -> bool {
        self.patterns
            .patterns
            .iter()
            .zip(&self.positions)
            .all(|(segments, positions)| positions.iter().all(|&p| p >= segments.len()))
    }
}

/// Matches `name` against `pattern`, where `*` stands for any run of
/// characters and every other character stands for itself.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let (pattern, name) = (pattern.as_bytes(), name.as_bytes());
    let (mut p, mut n) = (0, 0);
    // The most recent `*` and the name position it was tried at.
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, n));
            p += 1;
        } else if p < pattern.len() && pattern[p] == name[n] {
            p += 1;
            n += 1;
        } else if let Some((star_p, star_n)) = star {
            // Let the `*` take one more character and try again.
            star = Some((star_p, star_n + 1));
            p = star_p + 1;
            n = star_n + 1;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// Finds out whether any input directory or file matches a pattern, reading
/// only the `Directory` protos on paths that still can. Actions without a match keep
/// their ordinary input setup. Stops at the first match.
pub async fn has_matching_inputs(
    cas_store: &FastSlowStore,
    root_digest: DigestInfo,
    patterns: &PathPatterns,
    lease: Option<&dyn DigestLease>,
) -> Result<bool, Error> {
    let mut pending = vec![(root_digest, patterns.start())];
    while let Some((digest, states)) = pending.pop() {
        if let Some(lease) = lease {
            lease.acquire(&digest);
        }
        let directory = get_and_decode_digest::<Directory>(cas_store, digest.into()).await?;
        if directory
            .files
            .iter()
            .any(|file| states.descend(&file.name).is_match())
        {
            return Ok(true);
        }
        for child in directory.directories {
            let states = states.descend(&child.name);
            if states.is_match() {
                return Ok(true);
            }
            if !states.is_dead() {
                let digest = child
                    .digest
                    .as_ref()
                    .err_tip(|| "Missing input directory digest")?
                    .try_into()?;
                pending.push((digest, states));
            }
        }
    }
    Ok(false)
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
            selection: Selection::Exact(paths.iter().map(|path| root.join(path)).collect()),
            #[cfg(target_os = "linux")]
            root: root.to_owned(),
            prepared: Mutex::new(Vec::new()),
            prepared_files: Mutex::new(Vec::new()),
        }))
    }

    /// Creates state that selects directories by pattern as the input walk
    /// meets them, or returns `None` to use ordinary materialization when the
    /// command's working directory or outputs cannot be placed in the input
    /// root. A matching directory the command works inside, or that overlaps
    /// a declared output, is materialized the ordinary way; the rest of the
    /// action still reuses its other matches.
    pub fn for_patterns(
        cache: Arc<DirectoryCache>,
        patterns: Arc<PathPatterns>,
        root: &Path,
        command: &Command,
    ) -> Option<Arc<Self>> {
        let cwd = &command.working_directory;
        if !cwd.is_empty() && !is_canonical_relative(cwd) {
            return None;
        }
        let outputs = command
            .output_paths
            .iter()
            .chain(&command.output_files)
            .chain(&command.output_directories)
            .map(|output| resolve_output(cwd, output))
            .collect::<Option<Vec<_>>>()?;
        Some(Arc::new(Self {
            cache,
            selection: Selection::Patterns {
                patterns,
                root: root.to_owned(),
                working_directory: PathBuf::from(cwd),
                outputs,
            },
            #[cfg(target_os = "linux")]
            root: root.to_owned(),
            prepared: Mutex::new(Vec::new()),
            prepared_files: Mutex::new(Vec::new()),
        }))
    }

    fn selects(&self, path: &Path) -> bool {
        match &self.selection {
            Selection::Exact(targets) => targets.contains(path),
            Selection::Patterns {
                patterns,
                root,
                working_directory,
                outputs,
            } => path.strip_prefix(root).is_ok_and(|relative| {
                relative.components().next().is_some()
                    && patterns.matches(relative)
                    && !working_directory.starts_with(relative)
                    && !outputs.iter().any(|output| paths_overlap(output, relative))
            }),
        }
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
        if !self.selects(path) {
            return Ok(false);
        }
        let directory = self.cache.prepare_for_mount(digest, lease).await?;
        self.prepared.lock().push((path.to_owned(), directory));
        Ok(true)
    }

    /// Prepares and pins a tree holding the one input file at `path`, when a
    /// pattern selects that file, and returns where the file is in the cache
    /// for the caller to link to. Returns `None` when the path is not
    /// selected. Only patterns select files.
    pub async fn prepare_file(
        &self,
        path: &Path,
        digest: DigestInfo,
        is_executable: bool,
        lease: Option<&dyn DigestLease>,
    ) -> Result<Option<PathBuf>, Error> {
        if matches!(self.selection, Selection::Exact(_)) || !self.selects(path) {
            return Ok(None);
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Ok(None);
        };
        let directory = self
            .cache
            .prepare_file_for_mount(name, digest, is_executable, lease)
            .await?;
        // The link outlives the worker's current directory only if absolute.
        let target = nativelink_util::fs::canonicalize(directory.path())
            .await
            .err_tip(|| {
                format!(
                    "Resolving read-only input file {}",
                    directory.path().display()
                )
            })?
            .join(name);
        self.prepared_files.lock().push(directory);
        Ok(Some(target))
    }

    /// Resolve a program in a prepared mount before the child namespace exists.
    /// Validate symlinks against the pinned source, then return its path in the
    /// child workspace, not the cache path hidden by namespace setup.
    #[cfg(target_os = "linux")]
    pub fn executable_path(
        &self,
        cwd: &str,
        program: &std::ffi::OsStr,
    ) -> Result<Option<PathBuf>, Error> {
        let Some(program) = program.to_str() else {
            return Ok(None);
        };
        let Some(relative) = resolve_output(cwd, program) else {
            return Ok(None);
        };
        let candidate = self.root.join(relative);
        for (target, directory) in self.prepared.lock().iter() {
            let Ok(suffix) = candidate.strip_prefix(target) else {
                continue;
            };
            let source = directory.path().canonicalize()?;
            let executable = source.join(suffix).canonicalize().err_tip(|| {
                format!(
                    "Could not resolve executable in read-only input mount: {}",
                    candidate.display()
                )
            })?;
            let relative = executable.strip_prefix(&source).map_err(|_| {
                make_err!(
                    Code::InvalidArgument,
                    "Executable symlink escapes its read-only input mount: {}",
                    candidate.display()
                )
            })?;
            return Ok(Some(target.join(relative)));
        }
        Ok(None)
    }

    /// Replaces each prepared mount point with a symlink to its pinned cache
    /// entry. macOS has no bind mounts; a symlink is what lets the files keep
    /// the inode and change time its exec-time assessment cache is keyed on.
    /// The input walk left an empty directory at each target.
    #[cfg(target_os = "macos")]
    pub async fn link_into_workspace(&self) -> Result<(), Error> {
        use nativelink_util::fs;

        let links: Vec<(PathBuf, PathBuf)> = self
            .prepared
            .lock()
            .iter()
            .map(|(target, directory)| (target.clone(), directory.path().to_owned()))
            .collect();
        for (target, source) in links {
            // The link outlives the worker's current directory only if absolute.
            let source = fs::canonicalize(&source)
                .await
                .err_tip(|| format!("Resolving read-only input directory {}", source.display()))?;
            fs::remove_dir(&target)
                .await
                .err_tip(|| format!("Removing mount point {}", target.display()))?;
            fs::symlink(&source, &target).await.err_tip(|| {
                format!(
                    "Linking read-only input {} to {}",
                    target.display(),
                    source.display()
                )
            })?;
        }
        Ok(())
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
