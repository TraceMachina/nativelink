// Copyright 2024 Trace Machina, Inc. All rights reserved.
//
// Licensed under the Business Source License, Version 1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may request a copy of the License by emailing contact@nativelink.com.
//
// Use of this module requires an enterprise license agreement, which can be
// attained by emailing contact@nativelink.com or signing up for Nativelink
// Cloud at app.nativelink.com.
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! `PersistentWorkerPool`: per-`LocalWorker` pool of `LiveWorker`s keyed by
//! `WorkerKey`. Mirrors Bazel's notion of `WorkerKey` — two actions whose
//! executable + startup-flag prefix + wire format are identical share a worker
//! process.

use core::time::Duration;
use core::{fmt, mem};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_util::background_spawn;
use nativelink_util::task::JoinHandleDropGuard;
use parking_lot::Mutex;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use super::live_worker::{LiveWorker, Namespacing};
use super::protocol::WireFormat;

/// Default per-key bound on live workers. Matches Bazel's `worker_max_instances`
/// default and keeps memory pressure bounded without operator intervention.
pub const DEFAULT_MAX_WORKERS_PER_KEY: usize = 4;

/// Default idle timeout. A worker that has not handled a request for this
/// duration is shut down by the background sweeper.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_mins(5);

/// Default per-worker request cap. After this many `dispatch` calls a worker is
/// dropped on `release` (helps recycle accumulated JVM state).
pub const DEFAULT_MAX_REQUESTS_PER_WORKER: u64 = 200;

/// Default wait for a worker to come back when a key is at its cap.
pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

/// Identity by which two persistent-worker actions are considered compatible.
/// Same `WorkerKey` => same worker can serve both.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WorkerKey {
    /// Resolved tool executable. Mirrors `Action.arguments[0]`.
    pub executable: PathBuf,
    /// Leading arguments that become tool startup flags. Computed as the prefix
    /// of `Action.arguments[1..]` up to (excluding) the first arg starting
    /// with `@` (Bazel's response-file convention).
    pub startup_args: Vec<String>,
    /// Wire format from the action's `requires-worker-protocol`.
    pub wire_format: WireFormat,
    /// The action's environment, sorted by name. A worker process is
    /// started with exactly this environment, the way a one-shot action is,
    /// so two actions that differ in it do not share a process.
    pub env: Vec<(String, String)>,
    /// The namespaces the worker process runs in, as the action would get
    /// them, so actions that would be isolated differently never share one.
    pub namespacing: Namespacing,
}

/// The key is logged on every spawn and completion and quoted in errors the
/// client sees, so the environment shows as its variable names only: an
/// action's environment can carry a token.
impl fmt::Debug for WorkerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerKey")
            .field("executable", &self.executable)
            .field("startup_args", &self.startup_args)
            .field("wire_format", &self.wire_format)
            .field("namespacing", &self.namespacing)
            .field(
                "env",
                &self.env.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl WorkerKey {
    /// Derive a `WorkerKey` from a full action `argv` and the negotiated wire
    /// format.
    ///
    /// `argv[0]` is the executable. Subsequent arguments are scanned
    /// left-to-right and treated as startup flags until the first
    /// `@`-prefixed argument (Bazel's response file), at which point the rest
    /// are considered per-request inputs and excluded from the key.
    pub fn from_argv(argv: &[String], wire_format: WireFormat) -> Result<Self, Error> {
        let (exe, rest) = argv.split_first().ok_or_else(|| {
            make_err!(
                Code::InvalidArgument,
                "Cannot derive WorkerKey from empty argument list"
            )
        })?;
        let startup_args: Vec<String> = rest
            .iter()
            .take_while(|a| !a.starts_with('@'))
            .cloned()
            .collect();
        Ok(Self {
            executable: PathBuf::from(exe),
            startup_args,
            wire_format,
            env: Vec::new(),
            namespacing: Namespacing::None,
        })
    }

    /// The environment the worker process is started with, part of the key.
    #[must_use]
    pub fn with_env(mut self, mut env: Vec<(String, String)>) -> Self {
        env.sort();
        self.env = env;
        self
    }

    /// The namespaces the worker process runs in, part of the key.
    #[must_use]
    pub const fn with_namespacing(mut self, namespacing: Namespacing) -> Self {
        self.namespacing = namespacing;
        self
    }
}

/// Configuration knobs for a `PersistentWorkerPool`. Defaults are sensible for
/// a JVM-heavy workload; operators can override in `nativelink-config`.
#[derive(Clone, Copy, Debug)]
pub struct PoolConfig {
    pub max_workers_per_key: usize,
    pub idle_timeout: Duration,
    pub max_requests_per_worker: u64,
    /// SIGKILL grace when shutting a worker down.
    pub shutdown_grace: Duration,
    /// How long `acquire` waits for a worker to come back when the key is
    /// at `max_workers_per_key` with none idle, before the action runs
    /// one-shot instead. Bazel itself queues on `worker_max_instances`; a
    /// one-shot run of a worker tool pays the tool's startup again.
    pub acquire_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_workers_per_key: DEFAULT_MAX_WORKERS_PER_KEY,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            max_requests_per_worker: DEFAULT_MAX_REQUESTS_PER_WORKER,
            shutdown_grace: Duration::from_secs(5),
            acquire_timeout: DEFAULT_ACQUIRE_TIMEOUT,
        }
    }
}

/// Reservation handle returned by `acquire`. Holding it removes the worker
/// from the pool's idle set; `release` returns it (unless the request marked
/// it as dead). Drop-without-release is treated as "worker is dead": it is
/// shut down rather than re-pooled.
#[derive(Debug)]
pub struct Lease {
    inner: Option<LeaseInner>,
}

#[derive(Debug)]
struct LeaseInner {
    worker: LiveWorker,
    pool: Arc<PoolInner>,
    key: WorkerKey,
}

#[derive(Debug)]
struct CountSlotGuard {
    pool: Arc<PoolInner>,
    key: WorkerKey,
    active: bool,
}

impl CountSlotGuard {
    const fn new(pool: Arc<PoolInner>, key: WorkerKey) -> Self {
        Self {
            pool,
            key,
            active: true,
        }
    }

    fn disarm(mut self) {
        self.active = false;
    }

    fn decrement_now(mut self) {
        if self.active {
            self.pool.decrement_count(&self.key);
            self.active = false;
        }
    }
}

impl Drop for CountSlotGuard {
    fn drop(&mut self) {
        if self.active {
            self.pool.decrement_count(&self.key);
        }
    }
}

impl Lease {
    pub const fn worker(&mut self) -> &mut LiveWorker {
        &mut self
            .inner
            .as_mut()
            .expect("lease used after release")
            .worker
    }

    /// Return the worker to the pool. Caller signals whether the worker is
    /// still healthy. Unhealthy workers are shut down asynchronously and not
    /// returned to the idle set.
    pub async fn release(mut self, healthy: bool) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        inner
            .pool
            .return_worker(inner.key, inner.worker, healthy)
            .await;
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // If the caller forgot to call `release`, treat the worker as
        // unhealthy and shut it down on a detached task.
        if let Some(inner) = self.inner.take() {
            let pool = inner.pool.clone();
            let key = inner.key.clone();
            let worker = inner.worker;
            let grace = pool.config.shutdown_grace;
            background_spawn!("persistent_worker_lease_drop", async move {
                let count_slot = CountSlotGuard::new(pool.clone(), key.clone());
                warn!(
                    ?key,
                    "Lease dropped without release; treating worker as unhealthy"
                );
                worker.shutdown(grace).await;
                count_slot.decrement_now();
            });
        }
    }
}

/// Per-key pool state. Wrapped in a `Mutex` because `acquire` and `release`
/// run on different tokio tasks.
#[derive(Debug)]
struct KeyState {
    /// Idle workers ready to serve. Always size <= `total_count`.
    idle: Vec<LiveWorker>,
    /// Total live workers for this key (idle + leased). Used to enforce
    /// `max_workers_per_key`.
    total_count: usize,
}

impl KeyState {
    const fn new() -> Self {
        Self {
            idle: Vec::new(),
            total_count: 0,
        }
    }
}

#[derive(Debug)]
struct PoolInner {
    config: PoolConfig,
    state: Mutex<HashMap<WorkerKey, KeyState>>,
    /// Woken whenever a worker is returned or a slot is freed, for an
    /// `acquire` waiting on a key at its cap.
    returned: Notify,
}

impl PoolInner {
    /// Helper used by Drop on a forgotten Lease.
    fn decrement_count(&self, key: &WorkerKey) {
        {
            let mut state = self.state.lock();
            if let Some(s) = state.get_mut(key) {
                s.total_count = s.total_count.saturating_sub(1);
            }
        }
        self.returned.notify_waiters();
    }

    async fn return_worker(self: Arc<Self>, key: WorkerKey, worker: LiveWorker, healthy: bool) {
        let exceeded_cap = worker.request_count() >= self.config.max_requests_per_worker;
        if !healthy || exceeded_cap {
            let count_slot = CountSlotGuard::new(self.clone(), key.clone());
            debug!(
                ?key,
                request_count = worker.request_count(),
                healthy,
                "Retiring persistent worker"
            );
            worker.shutdown(self.config.shutdown_grace).await;
            count_slot.decrement_now();
            return;
        }
        {
            let mut state = self.state.lock();
            let entry = state.entry(key).or_insert_with(KeyState::new);
            entry.idle.push(worker);
        }
        self.returned.notify_waiters();
    }
}

/// The pool itself.
#[derive(Clone, Debug)]
pub struct PersistentWorkerPool {
    inner: Arc<PoolInner>,
}

impl Default for PersistentWorkerPool {
    fn default() -> Self {
        Self::new(PoolConfig::default())
    }
}

impl PersistentWorkerPool {
    /// How long `acquire` waits at the cap before giving up.
    pub fn acquire_timeout(&self) -> Duration {
        self.inner.config.acquire_timeout
    }

    pub fn new(config: PoolConfig) -> Self {
        Self {
            inner: Arc::new(PoolInner {
                config,
                state: Mutex::new(HashMap::new()),
                returned: Notify::new(),
            }),
        }
    }

    /// Acquire a worker for `key`. If an idle worker is available it is
    /// returned. Otherwise a new one is spawned, capped at
    /// `max_workers_per_key`. If the cap is reached and no idle worker exists,
    /// returns `Err(Code::ResourceExhausted)` — the caller should fall back to
    /// the one-shot subprocess path or wait and retry.
    ///
    /// `working_dir` is a stable worker process directory. Per-request action
    /// directories are passed through `WorkRequest::sandbox_dir`, because the
    /// action directory is deleted after each action completes.
    pub async fn acquire(
        &self,
        key: WorkerKey,
        executable_path: &Path,
        working_dir: &Path,
    ) -> Result<Lease, Error> {
        let deadline = tokio::time::Instant::now() + self.inner.config.acquire_timeout;
        loop {
            // Take an idle worker, or reserve a slot to spawn one, without
            // blocking on the spawn itself.
            let notified = {
                let mut state = self.inner.state.lock();
                let entry = state.entry(key.clone()).or_insert_with(KeyState::new);
                while let Some(mut worker) = entry.idle.pop() {
                    if worker.is_dead() {
                        // Process died while idle (e.g. JVM OOM). Drop it.
                        entry.total_count = entry.total_count.saturating_sub(1);
                        continue;
                    }
                    return Ok(Lease {
                        inner: Some(LeaseInner {
                            worker,
                            pool: self.inner.clone(),
                            key,
                        }),
                    });
                }
                if entry.total_count < self.inner.config.max_workers_per_key {
                    // Reserve a slot before releasing the lock so a concurrent
                    // acquire can't over-commit.
                    entry.total_count += 1;
                    break;
                }
                // At the cap with nothing idle: wait for a worker to come
                // back. The waiter is registered under the lock so a return
                // between the check and the wait is not missed.
                let notified = self.inner.returned.notified();
                let mut notified = Box::pin(notified);
                notified.as_mut().enable();
                notified
            };
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(make_err!(
                    Code::ResourceExhausted,
                    "Persistent worker pool for {key:?} is at capacity ({} workers) and none came back within {:?}",
                    self.inner.config.max_workers_per_key,
                    self.inner.config.acquire_timeout
                ));
            }
        }

        // Lock released; spawn outside the critical section.
        let count_slot = CountSlotGuard::new(self.inner.clone(), key.clone());
        let spawn_result = LiveWorker::spawn(
            executable_path,
            &key.startup_args,
            key.wire_format,
            working_dir,
            &key.env,
            key.namespacing,
        );
        let worker = match spawn_result {
            Ok(w) => w,
            Err(err) => {
                return Err(err).err_tip(|| format!("Spawning persistent worker for {key:?}"));
            }
        };
        count_slot.disarm();
        info!(?key, "Spawned new persistent worker");
        Ok(Lease {
            inner: Some(LeaseInner {
                worker,
                pool: self.inner.clone(),
                key,
            }),
        })
    }

    /// Runs `sweep_idle` on a timer until the pool is dropped. Without it an
    /// idle worker process, a JVM as a rule, lived as long as the worker did.
    pub fn spawn_sweeper(&self) -> JoinHandleDropGuard<()> {
        let inner = Arc::downgrade(&self.inner);
        let interval = (self.inner.config.idle_timeout / 4).max(Duration::from_secs(1));
        JoinHandleDropGuard::new(background_spawn!("persistent_worker_sweeper", async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                Self { inner }.sweep_idle().await;
            }
        }))
    }

    /// How many worker processes the pool holds, idle or leased.
    pub fn live_workers(&self) -> usize {
        self.inner
            .state
            .lock()
            .values()
            .map(|entry| entry.total_count)
            .sum()
    }

    /// Drop all idle workers whose `last_used` is older than `idle_timeout`.
    /// Intended to be called periodically from a background task.
    pub async fn sweep_idle(&self) {
        let threshold = std::time::Instant::now()
            .checked_sub(self.inner.config.idle_timeout)
            .unwrap_or_else(std::time::Instant::now);
        let evicted: Vec<LiveWorker> = {
            let mut state = self.inner.state.lock();
            let mut evicted = Vec::new();
            for entry in state.values_mut() {
                let (keep, drop_): (Vec<_>, Vec<_>) = mem::take(&mut entry.idle)
                    .into_iter()
                    .partition(|w| w.last_used() >= threshold);
                entry.total_count = entry.total_count.saturating_sub(drop_.len());
                entry.idle = keep;
                evicted.extend(drop_);
            }
            evicted
        };
        for w in evicted {
            w.shutdown(self.inner.config.shutdown_grace).await;
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::io::Write as _;

    #[cfg(unix)]
    use nativelink_macro::nativelink_test;

    use super::*;

    #[cfg(unix)]
    fn shell_script(working_dir: &Path, body: &str) -> PathBuf {
        let path = working_dir.join("worker.sh");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        file.sync_all().unwrap();
        path
    }

    /// The worker process gets only the environment in its key, so the
    /// scripts need PATH to find `sleep` wherever the tests run.
    #[cfg(unix)]
    fn path_env() -> Vec<(String, String)> {
        vec![(
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_default(),
        )]
    }

    #[cfg(unix)]
    fn shell_worker_key(script: &Path) -> WorkerKey {
        WorkerKey {
            executable: PathBuf::from("/bin/sh"),
            startup_args: vec![script.display().to_string()],
            wire_format: WireFormat::Json,
            env: path_env(),
            namespacing: Namespacing::None,
        }
    }

    #[test]
    fn worker_key_excludes_argfile_and_later_args() {
        let argv: Vec<String> = ["javac", "-source", "21", "@args.txt", "Foo.java"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let key = WorkerKey::from_argv(&argv, WireFormat::Proto).unwrap();
        assert_eq!(key.executable, PathBuf::from("javac"));
        assert_eq!(key.startup_args, vec!["-source".to_string(), "21".into()]);
    }

    #[test]
    fn worker_key_handles_empty_startup_args() {
        let argv: Vec<String> = ["javac", "@args.txt"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let key = WorkerKey::from_argv(&argv, WireFormat::Json).unwrap();
        assert_eq!(key.startup_args, Vec::<String>::new());
        assert_eq!(key.wire_format, WireFormat::Json);
    }

    #[test]
    fn worker_key_handles_no_argfile() {
        // No @argfile means *all* trailing args are startup flags. Unusual but
        // legal — the action just doesn't use a response file.
        let argv: Vec<String> = ["scalac", "-deprecation", "-Xfatal-warnings"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let key = WorkerKey::from_argv(&argv, WireFormat::Proto).unwrap();
        assert_eq!(key.startup_args.len(), 2);
    }

    #[test]
    fn worker_key_rejects_empty_argv() {
        let argv: Vec<String> = Vec::new();
        assert!(WorkerKey::from_argv(&argv, WireFormat::Proto).is_err());
    }

    #[test]
    fn worker_key_equality_collapses_compatible_actions() {
        let a = WorkerKey::from_argv(
            &["javac", "-source", "21", "@a"].map(String::from),
            WireFormat::Proto,
        )
        .unwrap();
        let b = WorkerKey::from_argv(
            &["javac", "-source", "21", "@b"].map(String::from),
            WireFormat::Proto,
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn worker_key_distinguishes_different_startup_args() {
        let a = WorkerKey::from_argv(
            &["javac", "-source", "21", "@a"].map(String::from),
            WireFormat::Proto,
        )
        .unwrap();
        let b = WorkerKey::from_argv(
            &["javac", "-source", "17", "@a"].map(String::from),
            WireFormat::Proto,
        )
        .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn worker_key_distinguishes_wire_formats() {
        let a =
            WorkerKey::from_argv(&["javac", "@a"].map(String::from), WireFormat::Proto).unwrap();
        let b = WorkerKey::from_argv(&["javac", "@a"].map(String::from), WireFormat::Json).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn worker_key_distinguishes_environments_and_ignores_their_order() {
        let base =
            || WorkerKey::from_argv(&["javac", "@a"].map(String::from), WireFormat::Proto).unwrap();
        let a = base().with_env(vec![
            ("PATH".into(), "/bin".into()),
            ("LANG".into(), "C".into()),
        ]);
        let b = base().with_env(vec![
            ("LANG".into(), "C".into()),
            ("PATH".into(), "/bin".into()),
        ]);
        let c = base().with_env(vec![("PATH".into(), "/usr/bin".into())]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn worker_key_distinguishes_namespacing() {
        let base =
            || WorkerKey::from_argv(&["javac", "@a"].map(String::from), WireFormat::Proto).unwrap();
        let namespaced =
            |isolate_network| base().with_namespacing(Namespacing::Yes { isolate_network });
        assert_eq!(base(), base().with_namespacing(Namespacing::None));
        assert_ne!(base(), namespaced(false));
        assert_ne!(namespaced(false), namespaced(true));
    }

    #[nativelink_test]
    #[cfg(unix)]
    async fn sweeper_retires_a_worker_idle_past_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let script = shell_script(dir.path(), "exec sleep 60\n");
        let key = shell_worker_key(&script);
        let pool = PersistentWorkerPool::new(PoolConfig {
            idle_timeout: Duration::from_millis(200),
            shutdown_grace: Duration::from_millis(100),
            ..PoolConfig::default()
        });
        let _sweeper = pool.spawn_sweeper();

        let lease = pool
            .acquire(key, Path::new("/bin/sh"), dir.path())
            .await
            .unwrap();
        lease.release(true).await;
        assert_eq!(pool.live_workers(), 1);

        // The sweeper runs every second at least; by then the worker has
        // been idle far longer than its timeout.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.live_workers() > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(pool.live_workers(), 0, "the idle worker was not retired");
    }

    #[nativelink_test]
    #[cfg(unix)]
    async fn dropping_unhealthy_release_frees_pool_slot() {
        let dir = tempfile::tempdir().unwrap();
        let script = shell_script(dir.path(), "exec sleep 60\n");
        let key = shell_worker_key(&script);
        let pool = PersistentWorkerPool::new(PoolConfig {
            max_workers_per_key: 1,
            shutdown_grace: Duration::from_secs(1),
            ..PoolConfig::default()
        });

        let lease = pool
            .acquire(key.clone(), Path::new("/bin/sh"), dir.path())
            .await
            .unwrap();
        let release_handle =
            background_spawn!("persistent_worker_release_test", lease.release(false));
        tokio::time::sleep(Duration::from_millis(50)).await;
        release_handle.abort();
        assert!(release_handle.await.unwrap_err().is_cancelled());

        let next_lease = tokio::time::timeout(
            Duration::from_secs(2),
            pool.acquire(key, Path::new("/bin/sh"), dir.path()),
        )
        .await
        .unwrap()
        .unwrap();
        next_lease.release(false).await;
    }

    #[nativelink_test]
    #[cfg(unix)]
    async fn acquire_at_the_cap_waits_for_a_returned_worker() {
        let dir = tempfile::tempdir().unwrap();
        let script = shell_script(dir.path(), "exec sleep 60\n");
        let key = shell_worker_key(&script);
        let pool = PersistentWorkerPool::new(PoolConfig {
            max_workers_per_key: 1,
            shutdown_grace: Duration::from_millis(100),
            acquire_timeout: Duration::from_secs(5),
            ..PoolConfig::default()
        });

        let lease = pool
            .acquire(key.clone(), Path::new("/bin/sh"), dir.path())
            .await
            .unwrap();
        let waiter = {
            let pool = pool.clone();
            let key = key.clone();
            let dir = dir.path().to_path_buf();
            background_spawn!("persistent_worker_acquire_waiter", async move {
                pool.acquire(key, Path::new("/bin/sh"), &dir).await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !waiter.is_finished(),
            "the second acquire should be waiting at the cap"
        );
        lease.release(true).await;
        let second = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("the waiter got the returned worker")
            .unwrap()
            .unwrap();
        second.release(false).await;
    }

    #[nativelink_test]
    #[cfg(unix)]
    async fn concurrent_acquire_respects_per_key_cap() {
        let dir = tempfile::tempdir().unwrap();
        let script = shell_script(dir.path(), "exec sleep 60\n");
        let key = shell_worker_key(&script);
        let pool = PersistentWorkerPool::new(PoolConfig {
            max_workers_per_key: 1,
            shutdown_grace: Duration::from_millis(100),
            acquire_timeout: Duration::from_millis(200),
            ..PoolConfig::default()
        });

        let lease = pool
            .acquire(key.clone(), Path::new("/bin/sh"), dir.path())
            .await
            .unwrap();
        let err = pool
            .acquire(key, Path::new("/bin/sh"), dir.path())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::ResourceExhausted);
        lease.release(false).await;
    }
}
