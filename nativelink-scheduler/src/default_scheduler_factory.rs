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

use std::sync::Arc;
use std::time::SystemTime;

use nativelink_config::schedulers::{
    ExperimentalSimpleSchedulerBackend, PropertyType, SchedulerSpec, SimpleSpec,
};
use nativelink_config::stores::EvictionPolicy;
use nativelink_error::{Error, ResultExt, make_input_err};
use nativelink_proto::com::github::trace_machina::nativelink::events::OriginEvent;
use nativelink_store::redis_store::{RedisStore, StandardRedisManager};
use nativelink_store::store_manager::StoreManager;
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::metrics::ActiveCountAttributes;
use redis::aio::ConnectionManager;
use tokio::sync::{Notify, mpsc};

use crate::cache_lookup_scheduler::CacheLookupScheduler;
use crate::grpc_scheduler::GrpcScheduler;
use crate::historical_resource_scheduler::HistoricalResourceScheduler;
use crate::known_platform_property_provider::KnownPlatformPropertyProvider;
use crate::memory_awaited_action_db::MemoryAwaitedActionDb;
use crate::property_modifier_scheduler::PropertyModifierScheduler;
use crate::simple_scheduler::SimpleScheduler;
use crate::store_awaited_action_db::StoreAwaitedActionDb;
use crate::worker_scheduler::WorkerScheduler;

/// Default timeout for recently completed actions in seconds.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_RETAIN_COMPLETED_FOR_S: u32 = 60;

pub type SchedulerFactoryResults = (
    Option<Arc<dyn KnownPlatformPropertyProvider>>,
    Option<Arc<dyn WorkerScheduler>>,
);

pub async fn scheduler_factory(
    spec: &SchedulerSpec,
    store_manager: &StoreManager,
    maybe_origin_event_tx: Option<&mpsc::Sender<OriginEvent>>,
) -> Result<SchedulerFactoryResults, Error> {
    inner_scheduler_factory(spec, store_manager, maybe_origin_event_tx).await
}

async fn inner_scheduler_factory(
    spec: &SchedulerSpec,
    store_manager: &StoreManager,
    maybe_origin_event_tx: Option<&mpsc::Sender<OriginEvent>>,
) -> Result<SchedulerFactoryResults, Error> {
    let scheduler: SchedulerFactoryResults = match spec {
        SchedulerSpec::Simple(spec) => {
            simple_scheduler_factory(spec, store_manager, SystemTime::now, maybe_origin_event_tx)
                .await?
        }
        SchedulerSpec::Grpc(spec) => (Some(Arc::new(GrpcScheduler::new(spec)?)), None),
        SchedulerSpec::CacheLookup(spec) => {
            let ac_store = store_manager
                .get_store(&spec.ac_store)
                .err_tip(|| format!("'ac_store': '{}' does not exist", spec.ac_store))?;
            let (action_scheduler, worker_scheduler) = Box::pin(inner_scheduler_factory(
                &spec.scheduler,
                store_manager,
                maybe_origin_event_tx,
            ))
            .await
            .err_tip(|| "In nested CacheLookupScheduler construction")?;
            let cache_lookup_scheduler = Arc::new(CacheLookupScheduler::new(
                ac_store,
                action_scheduler.err_tip(|| "Nested scheduler is not an action scheduler")?,
            )?);
            (Some(cache_lookup_scheduler), worker_scheduler)
        }
        SchedulerSpec::PropertyModifier(spec) => {
            let (action_scheduler, worker_scheduler) = Box::pin(inner_scheduler_factory(
                &spec.scheduler,
                store_manager,
                maybe_origin_event_tx,
            ))
            .await
            .err_tip(|| "In nested PropertyModifierScheduler construction")?;
            let property_modifier_scheduler = Arc::new(PropertyModifierScheduler::new(
                spec,
                action_scheduler.err_tip(|| "Nested scheduler is not an action scheduler")?,
            ));
            (Some(property_modifier_scheduler), worker_scheduler)
        }
        SchedulerSpec::HistoricalResource(spec) => {
            HistoricalResourceScheduler::validate(spec)?;
            // A number the scheduler writes into a property the nested
            // scheduler does not treat as a minimum is matched as an exact
            // string, and no worker advertises that string.
            if let SchedulerSpec::Simple(simple) = spec.scheduler.as_ref()
                && let Some(declared) = &simple.supported_platform_properties
            {
                let cold = spec.cold_start.unwrap_or_default();
                let dimensions = [
                    (
                        &spec.cpu_property_name,
                        spec.classes.iter().any(|c| c.cpu_count > 0) || cold.cpu_count > 0,
                    ),
                    (
                        &spec.memory_property_name,
                        spec.classes.iter().any(|c| c.memory_kb > 0) || cold.memory_kb > 0,
                    ),
                    (
                        &spec.disk_property_name,
                        spec.classes.iter().any(|c| c.disk_kb > 0) || cold.disk_kb > 0,
                    ),
                ];
                for (name, used) in dimensions {
                    if used && declared.get(name) != Some(&PropertyType::Minimum) {
                        return Err(make_input_err!(
                            "historical_resource reserves {name} but the nested scheduler does not declare it as a minimum property; add it to supported_platform_properties as minimum"
                        ));
                    }
                }
            }
            let (action_scheduler, worker_scheduler) = Box::pin(inner_scheduler_factory(
                &spec.scheduler,
                store_manager,
                maybe_origin_event_tx,
            ))
            .await
            .err_tip(|| "In nested HistoricalResourceScheduler construction")?;
            let historical_resource_scheduler = Arc::new(HistoricalResourceScheduler::new(
                spec,
                action_scheduler.err_tip(|| "Nested scheduler is not an action scheduler")?,
            ));
            (Some(historical_resource_scheduler), worker_scheduler)
        }
    };

    Ok(scheduler)
}

async fn simple_scheduler_factory(
    spec: &SimpleSpec,
    store_manager: &StoreManager,
    now_fn: fn() -> SystemTime,
    maybe_origin_event_tx: Option<&mpsc::Sender<OriginEvent>>,
) -> Result<SchedulerFactoryResults, Error> {
    if let Some(policy) = &spec.memory_escalation
        && policy.percent <= 100
    {
        return Err(make_input_err!(
            "memory_escalation.percent must be above 100 to grow the reservation, got {}",
            policy.percent
        ));
    }
    let active_count_attrs =
        ActiveCountAttributes::new(&spec.active_action_count_platform_properties)
            .err_tip(|| "In simple scheduler 'active_action_count_platform_properties'")?;
    match spec
        .experimental_backend
        .as_ref()
        .unwrap_or(&ExperimentalSimpleSchedulerBackend::Memory)
    {
        ExperimentalSimpleSchedulerBackend::Memory => {
            let task_change_notify = Arc::new(Notify::new());
            let awaited_action_db = memory_awaited_action_db_factory(
                spec.retain_completed_for_s,
                &task_change_notify,
                SystemTime::now,
                active_count_attrs,
            );
            let (action_scheduler, worker_scheduler) = SimpleScheduler::new(
                spec,
                awaited_action_db,
                task_change_notify,
                maybe_origin_event_tx.cloned(),
            );
            Ok((Some(action_scheduler), Some(worker_scheduler)))
        }
        ExperimentalSimpleSchedulerBackend::Redis(redis_config) => {
            let store = store_manager
                .get_store(redis_config.redis_store.as_ref())
                .err_tip(|| {
                    format!(
                        "'redis_store': '{}' does not exist",
                        redis_config.redis_store
                    )
                })?;
            let task_change_notify = Arc::new(Notify::new());
            let store = store
                .into_inner()
                .as_any_arc()
                .downcast::<RedisStore<ConnectionManager, StandardRedisManager<ConnectionManager>>>(
                )
                .map_err(|_| {
                    make_input_err!(
                        "Could not downcast to redis store in RedisAwaitedActionDb::new"
                    )
                })?;
            let awaited_action_db = StoreAwaitedActionDb::new(
                store,
                task_change_notify.clone(),
                now_fn,
                Default::default,
                // Passed through as 0, the store wrote every completed
                // record with no expiry (the Lua update treats 0 as
                // forever), so the scheduler's Redis kept every action ever
                // run and the search index grew without bound.
                retain_completed_for_s(spec.retain_completed_for_s),
                // Same normalisation SimpleScheduler applies, so the
                // keepalive stops being kept exactly when the timeout that
                // reads it comes due.
                if spec.client_action_timeout_s == 0 {
                    crate::simple_scheduler::DEFAULT_CLIENT_ACTION_TIMEOUT_S
                } else {
                    spec.client_action_timeout_s
                },
                spec.enable_active_action_count_metric,
                active_count_attrs,
            )
            .await
            .err_tip(|| "In state_manager_factory::redis_state_manager")?;
            let (action_scheduler, worker_scheduler) = SimpleScheduler::new(
                spec,
                awaited_action_db,
                task_change_notify,
                maybe_origin_event_tx.cloned(),
            );
            Ok((Some(action_scheduler), Some(worker_scheduler)))
        }
    }
}

/// How long a completed record is kept, whichever backend holds it: the
/// configured value, or the same default for both when it is unset.
#[must_use]
pub const fn retain_completed_for_s(configured: u32) -> u32 {
    if configured == 0 {
        DEFAULT_RETAIN_COMPLETED_FOR_S
    } else {
        configured
    }
}

pub fn memory_awaited_action_db_factory<I, NowFn>(
    configured_retain_completed_for_s: u32,
    task_change_notify: &Arc<Notify>,
    now_fn: NowFn,
    active_count_attrs: ActiveCountAttributes,
) -> MemoryAwaitedActionDb<I, NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Sync + 'static,
{
    MemoryAwaitedActionDb::new(
        &EvictionPolicy {
            max_seconds: retain_completed_for_s(configured_retain_completed_for_s),
            ..Default::default()
        },
        task_change_notify.clone(),
        now_fn,
        active_count_attrs,
    )
}
