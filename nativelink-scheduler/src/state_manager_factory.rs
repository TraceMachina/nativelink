use nativelink_config::schedulers::SchedulerBackend;
use nativelink_config::stores::EvictionPolicy;
use nativelink_util::instant_wrapper::InstantWrapper;
use tokio::sync::Notify;
use std::sync::Arc;
use nativelink_error::Error;

use nativelink_util::operation_state_manager::{
    WorkerStateManager,
    ClientStateManager,
    MatchingEngineStateManager
};

use crate::simple_scheduler_state_manager::SimpleSchedulerStateManager;
use crate::redis::redis_awaited_action_db::RedisAwaitedActionDb;
use crate::memory_awaited_action_db::MemoryAwaitedActionDb;

/// Default timeout for recently completed actions in seconds.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_RETAIN_COMPLETED_FOR_S: u32 = 60;

/// Default times a job can retry before failing.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_MAX_JOB_RETRIES: usize = 3;

pub type StateManagerFactoryResults = (
    Arc<dyn ClientStateManager>,
    Arc<dyn WorkerStateManager>,
    Arc<dyn MatchingEngineStateManager>,
);

fn memory_state_manager<I: InstantWrapper, NowFn: Fn() -> I + Clone + Send + Sync + 'static>(
    max_job_retries: usize,
    retain_completed_for_s: u32,
    tasks_or_worker_change_notify: Arc<Notify>,
    now_fn: NowFn
) -> Result<StateManagerFactoryResults, Error> {
    let state_manager = SimpleSchedulerStateManager::new(
        tasks_or_worker_change_notify,
        max_job_retries,
        MemoryAwaitedActionDb::new(
            &EvictionPolicy{
                max_seconds: retain_completed_for_s,
                ..Default::default()
            },
            now_fn,
        )
    );
    Ok((state_manager.clone(), state_manager.clone(), state_manager))
}

fn redis_state_manager(
    store_config: &nativelink_config::stores::RedisStore,
    max_job_retries: usize,
    tasks_or_worker_change_notify: Arc<Notify>,
) -> Result<StateManagerFactoryResults, Error> {
    let store = nativelink_store::redis_store::RedisStore::new(&store_config)?;
    let state_manager = SimpleSchedulerStateManager::new(
        tasks_or_worker_change_notify.clone(),
        max_job_retries,
        RedisAwaitedActionDb::new(store),
    );
    Ok((state_manager.clone(), state_manager.clone(), state_manager))
}

pub fn state_manager_factory<I: InstantWrapper, NowFn: Fn() -> I + Clone + Send + Sync + 'static>
(
    scheduler_cfg: &nativelink_config::schedulers::SimpleScheduler,
    tasks_or_worker_change_notify: Arc<Notify>,
    now_fn: NowFn
) -> Result<StateManagerFactoryResults, Error> {
    let mut max_job_retries = scheduler_cfg.max_job_retries;

    let mut retain_completed_for_s = scheduler_cfg.retain_completed_for_s;
    if retain_completed_for_s == 0 {
        retain_completed_for_s = DEFAULT_RETAIN_COMPLETED_FOR_S;
    }

    if max_job_retries == 0 {
        max_job_retries = DEFAULT_MAX_JOB_RETRIES;
    }

    match &scheduler_cfg.db_backend {
        SchedulerBackend::Memory => {
            memory_state_manager(max_job_retries, retain_completed_for_s, tasks_or_worker_change_notify, now_fn)
        },
        SchedulerBackend::Redis(store_config) => {
            redis_state_manager(store_config, max_job_retries, tasks_or_worker_change_notify)
        }
    }
}
