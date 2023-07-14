// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use std::cmp;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use futures::Future;
use lru::LruCache;
use parking_lot::Mutex;
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;
use tokio::time::Duration;

use action_messages::{ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState};
use common::log;
use error::{error_if, make_err, make_input_err, Code, Error, ResultExt};
use platform_property_manager::PlatformPropertyManager;
use scheduler::{ActionScheduler, WorkerScheduler};
use worker::{Worker, WorkerId, WorkerTimestamp, WorkerUpdate};

/// Default timeout for workers in seconds.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_WORKER_TIMEOUT_S: u64 = 5;

/// Default times a job can retry before failing.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_MAX_JOB_RETRIES: usize = 3;

/// An action that is being awaited on and last known state.
struct AwaitedAction {
    action_info: Arc<ActionInfo>,
    current_state: Arc<ActionState>,
    notify_channel: watch::Sender<Arc<ActionState>>,

    /// Number of attempts the job has been tried.
    attempts: usize,
    /// Possible last error set by the worker. If empty and attempts is set, it may be due to
    /// something like a worker timeout.
    last_error: Option<Error>,
}

/// Holds the relationship of a worker that is executing a specific action.
struct RunningAction {
    worker_id: WorkerId,
    action: AwaitedAction,
}

struct Workers {
    workers: LruCache<WorkerId, Worker>,
    /// The allocation strategy for workers.
    allocation_strategy: config::schedulers::WorkerAllocationStrategy,
}

impl Workers {
    fn new(allocation_strategy: config::schedulers::WorkerAllocationStrategy) -> Self {
        Self {
            workers: LruCache::unbounded(),
            allocation_strategy,
        }
    }

    /// Refreshes the lifetime of the worker with the given timestamp.
    fn refresh_lifetime(&mut self, worker_id: &WorkerId, timestamp: WorkerTimestamp) -> Result<(), Error> {
        let worker = self
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| make_input_err!("Worker not found in worker map in refresh_lifetime() {}", worker_id))?;
        error_if!(
            worker.last_update_timestamp > timestamp,
            "Worker already had a timestamp of {}, but tried to update it with {}",
            worker.last_update_timestamp,
            timestamp
        );
        worker.last_update_timestamp = timestamp;
        Ok(())
    }

    /// Adds a worker to the pool.
    /// Note: This function will not do any task matching.
    fn add_worker(&mut self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id;
        self.workers.put(worker_id, worker);

        // Worker is not cloneable, and we do not want to send the initial connection results until
        // we have added it to the map, or we might get some strange race conditions due to the way
        // the multi-threaded runtime works.
        let worker = self.workers.peek_mut(&worker_id).unwrap();
        let res = worker
            .send_initial_connection_result()
            .err_tip(|| "Failed to send initial connection result to worker");
        if let Err(e) = &res {
            log::error!(
                "Worker connection appears to have been closed while adding to pool : {:?}",
                e
            );
        }
        res
    }

    /// Removes worker from pool.
    /// Note: The caller is responsible for any rescheduling of any tasks that might be
    /// running.
    fn remove_worker(&mut self, worker_id: &WorkerId) -> Option<Worker> {
        self.workers.pop(worker_id)
    }

    /// Attempts to find a worker that is capable of running this action.
    // TODO(blaise.bruer) This algorithm is not very efficient. Simple testing using a tree-like
    // structure showed worse performance on a 10_000 worker * 7 properties * 1000 queued tasks
    // simulation of worst cases in a single threaded environment.
    fn find_worker_for_action_mut<'a>(&'a mut self, awaited_action: &AwaitedAction) -> Option<&'a mut Worker> {
        assert!(matches!(awaited_action.current_state.stage, ActionStage::Queued));
        let action_properties = &awaited_action.action_info.platform_properties;
        let mut workers_iter = self.workers.iter_mut();
        let workers_iter =
            match self.allocation_strategy {
                // Use rfind to get the least recently used that satisfies the properties.
                config::schedulers::WorkerAllocationStrategy::LeastRecentlyUsed => workers_iter
                    .rfind(|(_, w)| !w.is_paused && action_properties.is_satisfied_by(&w.platform_properties)),
                // Use find to get the most recently used that satisfies the properties.
                config::schedulers::WorkerAllocationStrategy::MostRecentlyUsed => workers_iter
                    .find(|(_, w)| !w.is_paused && action_properties.is_satisfied_by(&w.platform_properties)),
            };
        let worker_id = workers_iter.map(|(_, w)| &w.id);
        // We need to "touch" the worker to ensure it gets re-ordered in the LRUCache, since it was selected.
        if let Some(&worker_id) = worker_id {
            self.workers.get_mut(&worker_id)
        } else {
            None
        }
    }
}

struct SimpleSchedulerImpl {
    // BTreeMap uses `cmp` to do it's comparisons, this is a problem because we want to sort our
    // actions based on priority and insert timestamp but also want to find and join new actions
    // onto already executing (or queued) actions. We don't know the insert timestamp of queued
    // actions, so we won't be able to find it in a BTreeMap without iterating the entire map. To
    // get around this issue, we use two containers, one that will search using `Eq` which will
    // only match on the `unique_qualifier` field, which ignores fields that would prevent
    // multiplexing, and another which uses `Ord` for sorting.
    //
    // Important: These two fields must be kept in-sync, so if you modify one, you likely need to
    // modify the other.
    queued_actions_set: HashSet<Arc<ActionInfo>>,
    queued_actions: BTreeMap<Arc<ActionInfo>, AwaitedAction>,
    workers: Workers,
    active_actions: HashMap<Arc<ActionInfo>, RunningAction>,
    // These actions completed recently but had no listener, they might have
    // completed while the caller was thinking about calling wait_execution, so
    // keep their completion state around for a while to send back.
    recently_completed_actions: Vec<Arc<ActionState>>,
    /// Timeout of how long to evict workers if no response in this given amount of time in seconds.
    worker_timeout_s: u64,
    /// Default times a job can retry before failing.
    max_job_retries: usize,
    /// Notify task<->worker matching engine that work needs to be done.
    tasks_or_workers_change_notify: Arc<Notify>,
}

impl SimpleSchedulerImpl {
    fn subscribe_to_channel(awaited_action: &AwaitedAction) -> watch::Receiver<Arc<ActionState>> {
        let rx = awaited_action.notify_channel.subscribe();
        // TODO: Fix this when fixed upstream tokio-rs/tokio#5871
        awaited_action
            .notify_channel
            .send(awaited_action.current_state.clone())
            .unwrap();
        rx
    }

    /// Attempts to find a worker to execute an action and begins executing it.
    /// If an action is already running that is cacheable it may merge this action
    /// with the results and state changes of the already running action.
    /// If the task cannot be executed immediately it will be queued for execution
    /// based on priority and other metrics.
    /// All further updates to the action will be provided through `listener`.
    fn add_action(&mut self, action_info: ActionInfo) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        // Check to see if the action is running, if it is and cacheable, merge the actions.
        if let Some(running_action) = self.active_actions.get_mut(&action_info) {
            return Ok(Self::subscribe_to_channel(&running_action.action));
        }

        // Check to see if the action is queued, if it is and cacheable, merge the actions.
        if let Some(mut arc_action_info) = self.queued_actions_set.take(&action_info) {
            let (original_action_info, queued_action) = self
                .queued_actions
                .remove_entry(&arc_action_info)
                .err_tip(|| "Internal error queued_actions and queued_actions_set should match")?;

            let new_priority = cmp::max(original_action_info.priority, action_info.priority);
            drop(original_action_info); // This increases the chance Arc::make_mut won't copy.

            // In the event our task is higher priority than the one already scheduled, increase
            // the priority of the scheduled one.
            Arc::make_mut(&mut arc_action_info).priority = new_priority;

            let rx = queued_action.notify_channel.subscribe();
            // TODO: Fix this when fixed upstream tokio-rs/tokio#5871
            let _ = queued_action.notify_channel.send(queued_action.current_state.clone());

            // Even if we fail to send our action to the client, we need to add this action back to the
            // queue because it was remove earlier.
            self.queued_actions.insert(arc_action_info.clone(), queued_action);
            self.queued_actions_set.insert(arc_action_info);
            return Ok(rx);
        }

        // Action needs to be added to queue or is not cacheable.
        let action_info = Arc::new(action_info);

        let current_state = Arc::new(ActionState {
            unique_qualifier: action_info.unique_qualifier.clone(),
            stage: ActionStage::Queued,
        });

        let (tx, rx) = watch::channel(current_state.clone());
        self.queued_actions_set.insert(action_info.clone());
        self.queued_actions.insert(
            action_info.clone(),
            AwaitedAction {
                action_info,
                current_state,
                notify_channel: tx,
                attempts: 0,
                last_error: None,
            },
        );

        self.tasks_or_workers_change_notify.notify_one();
        Ok(rx)
    }

    fn clean_recently_completed_actions(&mut self) {
        // Keep for a minute maximum.
        let expiry_time = SystemTime::now().checked_sub(Duration::new(60, 0)).unwrap();
        self.recently_completed_actions.retain(|action| match &action.stage {
            ActionStage::Completed(state) => {
                if state.execution_metadata.worker_completed_timestamp > expiry_time {
                    true
                } else {
                    log::warn!(
                        "Action {} has no more listeners during update_action()",
                        action.action_digest().str()
                    );
                    false
                }
            }
            _ => false,
        });
    }

    fn find_recently_completed_action(
        &mut self,
        unique_qualifier: &ActionInfoHashKey,
    ) -> Option<watch::Receiver<Arc<ActionState>>> {
        let rx = self
            .recently_completed_actions
            .iter()
            .position(|state| state.unique_qualifier == *unique_qualifier)
            .map(|index| watch::channel(self.recently_completed_actions.swap_remove(index)).1);

        // Clean up the actions list.
        self.clean_recently_completed_actions();

        rx
    }

    // Gets an existing action by its name, very noddy implementation that just searches for it everywhere.
    fn find_existing_action(&self, unique_qualifier: &ActionInfoHashKey) -> Option<watch::Receiver<Arc<ActionState>>> {
        self.queued_actions
            .values()
            .chain(
                self.active_actions
                    .values()
                    .map(|running_action| &running_action.action),
            )
            .find_map(|awaited_action| {
                if awaited_action.current_state.unique_qualifier == *unique_qualifier {
                    Some(Self::subscribe_to_channel(awaited_action))
                } else {
                    None
                }
            })
    }

    fn retry_action(&mut self, action_info: &Arc<ActionInfo>, worker_id: &WorkerId) {
        match self.active_actions.remove(action_info) {
            Some(running_action) => {
                let mut awaited_action = running_action.action;
                let send_result = if awaited_action.attempts >= self.max_job_retries {
                    let mut default_action_result = ActionResult::default();
                    default_action_result.execution_metadata.worker = format!("{worker_id}");
                    Arc::make_mut(&mut awaited_action.current_state).stage = ActionStage::Error((
                        awaited_action.last_error.unwrap_or_else(|| {
                            make_err!(
                                Code::Internal,
                                "Job cancelled because it attempted to execute too many times and failed"
                            )
                        }),
                        default_action_result,
                    ));
                    awaited_action.notify_channel.send(awaited_action.current_state.clone())
                    // Do not put the action back in the queue here, as this action attempted to run too many
                    // times.
                } else {
                    Arc::make_mut(&mut awaited_action.current_state).stage = ActionStage::Queued;
                    let send_result = awaited_action.notify_channel.send(awaited_action.current_state.clone());
                    self.queued_actions_set.insert(action_info.clone());
                    self.queued_actions.insert(action_info.clone(), awaited_action);
                    send_result
                };

                if send_result.is_err() {
                    // Don't remove this task, instead we keep them around for a bit just in case
                    // the client disconnected and will reconnect and ask for same job to be executed
                    // again.
                    log::warn!(
                        "Action {} has no more listeners during evict_worker()",
                        action_info.digest().str()
                    );
                }
            }
            None => {
                log::error!("Worker stated it was running an action, but it was not in the active_actions : Worker: {:?}, ActionInfo: {:?}", worker_id, action_info);
            }
        }
    }

    /// Evicts the worker from the pool and puts items back into the queue if anything was being executed on it.
    fn immediate_evict_worker(&mut self, worker_id: &WorkerId) {
        if let Some(mut worker) = self.workers.remove_worker(worker_id) {
            // We don't care if we fail to send message to worker, this is only a best attempt.
            let _ = worker.notify_update(WorkerUpdate::Disconnect);
            // We create a temporary Vec to avoid doubt about a possible code
            // path touching the worker.running_action_infos elsewhere.
            for action_info in worker.running_action_infos.drain() {
                self.retry_action(&action_info, worker_id);
            }
        }
        // Note: Calling this many time is very cheap, it'll only trigger `do_try_match` once.
        self.tasks_or_workers_change_notify.notify_one();
    }

    // TODO(blaise.bruer) This is an O(n*m) (aka n^2) algorithm. In theory we can create a map
    // of capabilities of each worker and then try and match the actions to the worker using
    // the map lookup (ie. map reduce).
    fn do_try_match(&mut self) {
        // TODO(blaise.bruer) This is a bit difficult because of how rust's borrow checker gets in
        // the way. We need to conditionally remove items from the `queued_action`. Rust is working
        // to add `drain_filter`, which would in theory solve this problem, but because we need
        // to iterate the items in reverse it becomes more difficult (and it is currently an
        // unstable feature [see: https://github.com/rust-lang/rust/issues/70530]).
        let action_infos: Vec<Arc<ActionInfo>> = self.queued_actions.keys().rev().cloned().collect();
        for action_info in action_infos {
            let Some(awaited_action) = self.queued_actions.get(action_info.as_ref()) else {
                log::error!(
                    "queued_actions out of sync with itself for action {}",
                    action_info.digest().str()
                );
                continue;
            };
            let Some(worker) = self.workers.find_worker_for_action_mut(awaited_action) else {
                // No worker found, check the next action to see if there's a
                // matching one for that.
                continue;
            };
            let worker_id = worker.id;

            // Try to notify our worker of the new action to run, if it fails remove the worker from the
            // pool and try to find another worker.
            let notify_worker_result = worker.notify_update(WorkerUpdate::RunAction(action_info.clone()));
            if notify_worker_result.is_err() {
                // Remove worker, as it is no longer receiving messages and let it try to find another worker.
                log::warn!("Worker command failed, removing worker {}", worker_id);
                self.immediate_evict_worker(&worker_id);
                return;
            }

            // At this point everything looks good, so remove it from the queue and add it to active actions.
            let (action_info, mut awaited_action) = self.queued_actions.remove_entry(action_info.as_ref()).unwrap();
            assert!(
                self.queued_actions_set.remove(&action_info),
                "queued_actions_set should always have same keys as queued_actions"
            );
            Arc::make_mut(&mut awaited_action.current_state).stage = ActionStage::Executing;
            let send_result = awaited_action.notify_channel.send(awaited_action.current_state.clone());
            if send_result.is_err() {
                // Don't remove this task, instead we keep them around for a bit just in case
                // the client disconnected and will reconnect and ask for same job to be executed
                // again.
                log::warn!(
                    "Action {} has no more listeners",
                    awaited_action.action_info.digest().str()
                );
            }
            awaited_action.attempts += 1;
            self.active_actions.insert(
                action_info.clone(),
                RunningAction {
                    worker_id,
                    action: awaited_action,
                },
            );
        }
    }

    fn update_action_with_internal_error(
        &mut self,
        worker_id: &WorkerId,
        action_info_hash_key: &ActionInfoHashKey,
        err: Error,
    ) {
        let Some((action_info, mut running_action)) = self.active_actions.remove_entry(action_info_hash_key) else {
            log::error!("Could not find action info in active actions : {action_info_hash_key:?}");
            return;
        };

        let due_to_backpressure = err.code == Code::ResourceExhausted;
        // Don't count a backpressure failure as an attempt for an action.
        if due_to_backpressure {
            running_action.action.attempts -= 1;
        }

        if running_action.worker_id == *worker_id {
            // Don't set the error on an action that's running somewhere else.
            log::warn!("Internal error for worker {}: {}", worker_id, err);
            running_action.action.last_error = Some(err);
        } else {
            log::error!(
                "Got a result from a worker that should not be running the action, Removing worker. Expected worker {} got worker {}",
                    running_action.worker_id, worker_id
            );
        }

        // Now put it back. retry_action() needs it to be there to send errors properly.
        self.active_actions.insert(action_info.clone(), running_action);

        // Clear this action from the current worker.
        if let Some(worker) = self.workers.workers.get_mut(worker_id) {
            let was_paused = worker.is_paused;
            // This unpauses, but since we're completing with an error, don't
            // unpause unless all actions have completed.
            worker.complete_action(&action_info);
            // Only pause if there's an action still waiting that will unpause.
            if (was_paused || due_to_backpressure) && worker.has_actions() {
                worker.is_paused = true;
            }
        }

        // Re-queue the action or fail on max attempts.
        self.retry_action(&action_info, worker_id);
    }

    fn update_action(
        &mut self,
        worker_id: &WorkerId,
        action_info_hash_key: &ActionInfoHashKey,
        action_stage: ActionStage,
    ) -> Result<(), Error> {
        if !action_stage.has_action_result() {
            let msg = format!("Worker '{worker_id}' set the action_stage of running action {action_info_hash_key:?} to {action_stage:?}. Removing worker.");
            log::error!("{}", msg);
            self.immediate_evict_worker(worker_id);
            return Err(make_input_err!("{}", msg));
        }

        let (action_info, mut running_action) = self
            .active_actions
            .remove_entry(action_info_hash_key)
            .err_tip(|| format!("Could not find action info in active actions : {action_info_hash_key:?}"))?;

        if running_action.worker_id != *worker_id {
            let msg = format!(
                "Got a result from a worker that should not be running the action, {}",
                format_args!(
                    "Removing worker. Expected worker {} got worker {}",
                    running_action.worker_id, worker_id
                )
            );
            log::error!("{}", msg);
            // First put it back in our active_actions or we will drop the task.
            self.active_actions.insert(action_info, running_action);
            self.immediate_evict_worker(worker_id);
            return Err(make_input_err!("{}", msg));
        }

        Arc::make_mut(&mut running_action.action.current_state).stage = action_stage;

        let send_result = running_action
            .action
            .notify_channel
            .send(running_action.action.current_state.clone());

        if !running_action.action.current_state.stage.is_finished() {
            if send_result.is_err() {
                log::warn!(
                    "Action {} has no more listeners during update_action()",
                    action_info.digest().str()
                );
            }
            // If the operation is not finished it means the worker is still working on it, so put it
            // back or else we will loose track of the task.
            self.active_actions.insert(action_info, running_action);
            return Ok(());
        } else if send_result.is_err() {
            // Keep in case this is asked for soon.
            self.recently_completed_actions
                .push(running_action.action.current_state);
            // Clean up any old actions.
            self.clean_recently_completed_actions();
        }

        let worker = self
            .workers
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| make_input_err!("WorkerId '{}' does not exist in workers map", worker_id))?;
        worker.complete_action(&action_info);
        self.tasks_or_workers_change_notify.notify_one();

        Ok(())
    }
}

/// Engine used to manage the queued/running tasks and relationship with
/// the worker nodes. All state on how the workers and actions are interacting
/// should be held in this struct.
pub struct SimpleScheduler {
    inner: Arc<Mutex<SimpleSchedulerImpl>>,
    platform_property_manager: Arc<PlatformPropertyManager>,
    task_worker_matching_future: JoinHandle<()>,
}

impl SimpleScheduler {
    #[inline]
    #[must_use]
    pub fn new(scheduler_cfg: &config::schedulers::SimpleScheduler) -> Self {
        Self::new_with_callback(scheduler_cfg, || {
            // The cost of running `do_try_match()` is very high, but constant
            // in relation to the number of changes that have happened. This means
            // that grabbing this lock to process `do_try_match()` should always
            // yield to any other tasks that might want the lock. The easiest and
            // most fair way to do this is to sleep for a small amount of time.
            // Using something like tokio::task::yield_now() does not yield as
            // aggresively as we'd like if new futures are scheduled within a future.
            tokio::time::sleep(Duration::from_millis(1))
        })
    }

    pub fn new_with_callback<Fut: Future<Output = ()> + Send, F: Fn() -> Fut + Send + Sync + 'static>(
        scheduler_cfg: &config::schedulers::SimpleScheduler,
        on_matching_engine_run: F,
    ) -> Self {
        let platform_property_manager = Arc::new(PlatformPropertyManager::new(
            scheduler_cfg.supported_platform_properties.clone().unwrap_or_default(),
        ));

        let mut worker_timeout_s = scheduler_cfg.worker_timeout_s;
        if worker_timeout_s == 0 {
            worker_timeout_s = DEFAULT_WORKER_TIMEOUT_S;
        }

        let mut max_job_retries = scheduler_cfg.max_job_retries;
        if max_job_retries == 0 {
            max_job_retries = DEFAULT_MAX_JOB_RETRIES;
        }

        let tasks_or_workers_change_notify = Arc::new(Notify::new());

        let inner = Arc::new(Mutex::new(SimpleSchedulerImpl {
            queued_actions_set: HashSet::new(),
            queued_actions: BTreeMap::new(),
            workers: Workers::new(scheduler_cfg.allocation_strategy),
            active_actions: HashMap::new(),
            recently_completed_actions: Vec::new(),
            worker_timeout_s,
            max_job_retries,
            tasks_or_workers_change_notify: tasks_or_workers_change_notify.clone(),
        }));
        let weak_inner = Arc::downgrade(&inner);
        Self {
            inner,
            platform_property_manager,
            task_worker_matching_future: tokio::spawn(async move {
                // Break out of the loop only when the inner is dropped.
                loop {
                    tasks_or_workers_change_notify.notified().await;
                    match weak_inner.upgrade() {
                        // Note: According to `parking_lot` documentation, the default
                        // `Mutex` implementation is eventual fairness, so we don't
                        // really need to worry about this thread taking the lock
                        // starving other threads too much.
                        Some(inner_mux) => {
                            let mut inner = inner_mux.lock();
                            inner.do_try_match();
                        }
                        // If the inner went away it means the scheduler is shutting
                        // down, so we need to resolve our future.
                        None => return,
                    };
                    on_matching_engine_run().await;
                }
                // Unreachable.
            }),
        }
    }

    /// Checks to see if the worker exists in the worker pool. Should only be used in unit tests.
    #[must_use]
    pub fn contains_worker_for_test(&self, worker_id: &WorkerId) -> bool {
        let inner = self.inner.lock();
        inner.workers.workers.contains(worker_id)
    }

    /// A unit test function used to send the keep alive message to the worker from the server.
    pub fn send_keep_alive_to_worker_for_test(&self, worker_id: &WorkerId) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        let worker = inner
            .workers
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| make_input_err!("WorkerId '{}' does not exist in workers map", worker_id))?;
        worker.keep_alive()
    }
}

#[async_trait]
impl ActionScheduler for SimpleScheduler {
    async fn get_platform_property_manager(&self, _instance_name: &str) -> Result<Arc<PlatformPropertyManager>, Error> {
        Ok(self.platform_property_manager.clone())
    }

    async fn add_action(&self, action_info: ActionInfo) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        let mut inner = self.inner.lock();
        inner.add_action(action_info)
    }

    async fn find_existing_action(
        &self,
        unique_qualifier: &ActionInfoHashKey,
    ) -> Option<watch::Receiver<Arc<ActionState>>> {
        let mut inner = self.inner.lock();
        inner
            .find_existing_action(unique_qualifier)
            .or_else(|| inner.find_recently_completed_action(unique_qualifier))
    }
}

#[async_trait]
impl WorkerScheduler for SimpleScheduler {
    fn get_platform_property_manager(&self) -> &PlatformPropertyManager {
        self.platform_property_manager.as_ref()
    }

    async fn add_worker(&self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id;
        let mut inner = self.inner.lock();
        let res = inner
            .workers
            .add_worker(worker)
            .err_tip(|| "Error while adding worker, removing from pool");
        if res.is_err() {
            inner.immediate_evict_worker(&worker_id);
        }
        inner.tasks_or_workers_change_notify.notify_one();
        res
    }

    async fn update_action_with_internal_error(
        &self,
        worker_id: &WorkerId,
        action_info_hash_key: &ActionInfoHashKey,
        err: Error,
    ) {
        let mut inner = self.inner.lock();
        inner.update_action_with_internal_error(worker_id, action_info_hash_key, err);
    }

    async fn update_action(
        &self,
        worker_id: &WorkerId,
        action_info_hash_key: &ActionInfoHashKey,
        action_stage: ActionStage,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        inner.update_action(worker_id, action_info_hash_key, action_stage)
    }

    async fn worker_keep_alive_received(&self, worker_id: &WorkerId, timestamp: WorkerTimestamp) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        inner
            .workers
            .refresh_lifetime(worker_id, timestamp)
            .err_tip(|| "Error refreshing lifetime in worker_keep_alive_received()")
    }

    async fn remove_worker(&self, worker_id: WorkerId) {
        let mut inner = self.inner.lock();
        inner.immediate_evict_worker(&worker_id);
    }

    async fn remove_timedout_workers(&self, now_timestamp: WorkerTimestamp) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        // Items should be sorted based on last_update_timestamp, so we don't need to iterate the entire
        // map most of the time.
        let worker_ids_to_remove: Vec<WorkerId> = inner
            .workers
            .workers
            .iter()
            .rev()
            .map_while(|(worker_id, worker)| {
                if worker.last_update_timestamp <= now_timestamp - inner.worker_timeout_s {
                    Some(*worker_id)
                } else {
                    None
                }
            })
            .collect();
        for worker_id in &worker_ids_to_remove {
            log::warn!("Worker {} timed out, removing from pool", worker_id);
            inner.immediate_evict_worker(worker_id);
        }
        Ok(())
    }
}

impl Drop for SimpleScheduler {
    fn drop(&mut self) {
        self.task_worker_matching_future.abort();
    }
}
