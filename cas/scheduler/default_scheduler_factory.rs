// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::Future;
use tokio::time::interval;

use cache_lookup_scheduler::CacheLookupScheduler;
use config::schedulers::SchedulerConfig;
use error::{Error, ResultExt};
use grpc_scheduler::GrpcScheduler;
use scheduler::{ActionScheduler, WorkerScheduler};
use simple_scheduler::SimpleScheduler;
use store::StoreManager;

pub type SchedulerFactoryResults = (Option<Arc<dyn ActionScheduler>>, Option<Arc<dyn WorkerScheduler>>);

pub fn scheduler_factory<'a>(
    scheduler_type_cfg: &'a SchedulerConfig,
    store_manager: &'a StoreManager,
) -> Pin<Box<dyn Future<Output = Result<SchedulerFactoryResults, Error>> + 'a>> {
    Box::pin(async move {
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
                let (action_scheduler, worker_scheduler) = scheduler_factory(&config.scheduler, store_manager).await?;
                let cache_lookup_scheduler = Arc::new(CacheLookupScheduler::new(
                    cas_store,
                    ac_store,
                    action_scheduler.err_tip(|| "Nested scheduler is not an action scheduler")?,
                )?);
                (Some(cache_lookup_scheduler), worker_scheduler)
            }
        };

        if let Some(action_scheduler) = &scheduler.0 {
            start_cleanup_timer(action_scheduler);
        }

        Ok(scheduler)
    })
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
