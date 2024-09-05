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

use std::sync::Arc;

use nativelink_config::schedulers::SchedulerBackend;
use nativelink_config::stores::EvictionPolicy;
use nativelink_error::{Error, ResultExt};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::operation_state_manager::{
    ClientStateManager, MatchingEngineStateManager, WorkerStateManager,
};
use nativelink_util::store_trait::StoreLike;
use tokio::spawn;
use tokio::sync::Notify;

use crate::memory_awaited_action_db::MemoryAwaitedActionDb;
use crate::redis::redis_awaited_action_db::RedisAwaitedActionDb;
use crate::simple_scheduler_state_manager::SimpleSchedulerStateManager;

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

pub fn state_manager_factory<
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Sync + 'static,
>(
    scheduler_cfg: &nativelink_config::schedulers::SimpleScheduler,
    tasks_or_workers_change_notify: Arc<Notify>,
    now_fn: NowFn,
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
        SchedulerBackend::memory => {
            let state_manager = SimpleSchedulerStateManager::new(
                tasks_or_workers_change_notify,
                max_job_retries,
                MemoryAwaitedActionDb::new(
                    &EvictionPolicy {
                        max_seconds: retain_completed_for_s,
                        ..Default::default()
                    },
                    now_fn,
                ),
            );
            Ok((state_manager.clone(), state_manager.clone(), state_manager))
        }
        SchedulerBackend::redis(store_config) => {
            println!("redis config - {store_config:?}");
            let store = nativelink_store::redis_store::RedisStore::new(store_config)
                .err_tip(|| "In state_manager_factory::redis_state_manager")?;
            let state_manager = SimpleSchedulerStateManager::new(
                tasks_or_workers_change_notify.clone(),
                max_job_retries,
                RedisAwaitedActionDb::new(store, tasks_or_workers_change_notify),
            );
            Ok((state_manager.clone(), state_manager.clone(), state_manager))
        }
    }
}
