// Copyright 2023 The Native Link Authors. All rights reserved.
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

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::interval;

use cache_lookup_scheduler::CacheLookupScheduler;
use config::schedulers::SchedulerConfig;
use error::{Error, ResultExt};
use grpc_scheduler::GrpcScheduler;
use metrics_utils::Registry;
use property_modifier_scheduler::PropertyModifierScheduler;
use scheduler::{ActionScheduler, WorkerScheduler};
use simple_scheduler::SimpleScheduler;
use store::StoreManager;

pub type SchedulerFactoryResults = (Option<Arc<dyn ActionScheduler>>, Option<Arc<dyn WorkerScheduler>>);

pub fn scheduler_factory(
    scheduler_type_cfg: &SchedulerConfig,
    store_manager: &StoreManager,
    scheduler_metrics: &mut Registry,
) -> Result<SchedulerFactoryResults, Error> {
    let mut visited_schedulers = HashSet::new();
    inner_scheduler_factory(
        scheduler_type_cfg,
        store_manager,
        Some(scheduler_metrics),
        &mut visited_schedulers,
    )
}

fn inner_scheduler_factory(
    scheduler_type_cfg: &SchedulerConfig,
    store_manager: &StoreManager,
    maybe_scheduler_metrics: Option<&mut Registry>,
    visited_schedulers: &mut HashSet<usize>,
) -> Result<SchedulerFactoryResults, Error> {
    let scheduler: SchedulerFactoryResults = match scheduler_type_cfg {
        SchedulerConfig::simple(config) => {
            let scheduler = Arc::new(SimpleScheduler::new(config));
            (Some(scheduler.clone()), Some(scheduler))
        }
        SchedulerConfig::grpc(config) => (Some(Arc::new(GrpcScheduler::new(config)?)), None),
        SchedulerConfig::cache_lookup(config) => {
            let cas_store = store_manager
                .get_store(&config.cas_store)
                .err_tip(|| format!("'cas_store': '{}' does not exist", config.cas_store))?;
            let ac_store = store_manager
                .get_store(&config.ac_store)
                .err_tip(|| format!("'ac_store': '{}' does not exist", config.ac_store))?;
            let (action_scheduler, worker_scheduler) =
                inner_scheduler_factory(&config.scheduler, store_manager, None, visited_schedulers)
                    .err_tip(|| "In nested CacheLookupScheduler construction")?;
            let cache_lookup_scheduler = Arc::new(CacheLookupScheduler::new(
                cas_store,
                ac_store,
                action_scheduler.err_tip(|| "Nested scheduler is not an action scheduler")?,
            )?);
            (Some(cache_lookup_scheduler), worker_scheduler)
        }
        SchedulerConfig::property_modifier(config) => {
            let (action_scheduler, worker_scheduler) =
                inner_scheduler_factory(&config.scheduler, store_manager, None, visited_schedulers)
                    .err_tip(|| "In nested PropertyModifierScheduler construction")?;
            let property_modifier_scheduler = Arc::new(PropertyModifierScheduler::new(
                config,
                action_scheduler.err_tip(|| "Nested scheduler is not an action scheduler")?,
            ));
            (Some(property_modifier_scheduler), worker_scheduler)
        }
    };

    if let Some(scheduler_metrics) = maybe_scheduler_metrics {
        if let Some(action_scheduler) = &scheduler.0 {
            start_cleanup_timer(action_scheduler);
            // We need a way to prevent our scheduler form having `register_metrics()` called multiple times.
            // This is the equivalent of grabbing a uintptr_t in C++, storing it in a set, and checking if it's
            // already been visited. We can't use the Arc's pointer directly because it has two interfaces
            // (ActionScheduler and WorkerScheduler) and we need to be able to know if the underlying scheduler
            // has already been visited, not just the trait. `Any` could be used, but that'd require some rework
            // of all the schedulers. This is the most simple way to do it. Rust's uintptr_t is usize.
            let action_scheduler_uintptr: usize = Arc::as_ptr(action_scheduler).cast::<()>() as usize;
            if !visited_schedulers.contains(&action_scheduler_uintptr) {
                visited_schedulers.insert(action_scheduler_uintptr);
                action_scheduler.clone().register_metrics(scheduler_metrics);
            }
        }
        if let Some(worker_scheduler) = &scheduler.1 {
            let worker_scheduler_uintptr: usize = Arc::as_ptr(worker_scheduler).cast::<()>() as usize;
            if !visited_schedulers.contains(&worker_scheduler_uintptr) {
                visited_schedulers.insert(worker_scheduler_uintptr);
                worker_scheduler.clone().register_metrics(scheduler_metrics);
            }
            worker_scheduler.clone().register_metrics(scheduler_metrics);
        }
    }

    Ok(scheduler)
}

fn start_cleanup_timer(action_scheduler: &Arc<dyn ActionScheduler>) {
    let weak_scheduler = Arc::downgrade(action_scheduler);
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            match weak_scheduler.upgrade() {
                Some(scheduler) => scheduler.clean_recently_completed_actions().await,
                // If we fail to upgrade, our service is probably destroyed, so return.
                None => return,
            }
        }
    });
}
