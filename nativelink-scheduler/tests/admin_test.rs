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

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use futures::stream;
use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::{PropertyType, SimpleSpec};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::UpdateForWorker;
use nativelink_scheduler::admin::{QueuedDemand, queued_demand, queued_demand_json, workers_json};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::{WorkerScheduler, WorkerSummary};
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionState, OperationId, WorkerId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW_TIME: u64 = 10000;

fn make_scheduler() -> Arc<SimpleScheduler> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
        &SimpleSpec {
            supported_platform_properties: Some(HashMap::from([(
                "cpu_count".to_string(),
                PropertyType::Minimum,
            )])),
            client_action_timeout_s: 1_000_000,
            ..SimpleSpec::default()
        },
        memory_awaited_action_db_factory(0, &task_change_notify, MockInstantWrapped::default),
        || async move {},
        task_change_notify,
        MockInstantWrapped::default,
        None,
    );
    scheduler
}

async fn add_worker(
    scheduler: &SimpleScheduler,
    name: &str,
    cpu_count: u64,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::channel(64);
    let worker = Worker::new(
        WorkerId(name.to_string()),
        PlatformProperties::new(HashMap::from([(
            "cpu_count".to_string(),
            PlatformPropertyValue::Minimum(cpu_count),
        )])),
        tx,
        NOW_TIME,
        4,
    );
    scheduler
        .add_worker(worker)
        .await
        .err_tip(|| "Failed to add worker")?;
    tokio::task::yield_now().await;
    rx.recv().await.expect("the connection result");
    Ok(rx)
}

async fn add_action(
    scheduler: &SimpleScheduler,
    digest_byte: u8,
    priority: i32,
    cpu_count: &str,
) -> Result<Box<dyn ActionStateResult>, Error> {
    let mut action_info = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([digest_byte; 32], 512),
    );
    let inner = Arc::make_mut(&mut action_info);
    inner.priority = priority;
    inner.platform_properties = HashMap::from([("cpu_count".to_string(), cpu_count.to_string())]);
    let result = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;
    Ok(result)
}

/// The demand listing is what a provisioner scales on: every queued action,
/// highest priority first, with what it asked for and when its client was
/// last heard from.
#[nativelink_test]
async fn queued_demand_lists_waiting_actions_highest_priority_first() -> Result<(), Error> {
    let scheduler = make_scheduler();
    let _low = add_action(&scheduler, 1, 0, "2").await?;
    let _high = add_action(&scheduler, 2, 5, "1").await?;

    let demand = queued_demand(scheduler.as_ref()).await?;
    assert_eq!(demand.len(), 2);
    assert_eq!(demand[0].priority, 5);
    assert_eq!(
        demand[0].platform_properties,
        HashMap::from([("cpu_count".to_string(), "1".to_string())])
    );
    assert_eq!(demand[1].priority, 0);
    for entry in &demand {
        assert_eq!(entry.queued_since_ms, NOW_TIME * 1000);
        let last_seen = entry
            .client_last_seen_ms
            .expect("a client is still waiting on the action");
        assert!(last_seen >= entry.queued_since_ms);
        assert_ne!(entry.operation_id, "");
    }
    Ok(())
}

/// An action a worker is running is no longer demand, and the worker
/// listing shows it running with the room it took.
#[nativelink_test]
async fn running_actions_leave_the_demand_and_show_on_the_worker() -> Result<(), Error> {
    let scheduler = make_scheduler();
    let _rx = add_worker(&scheduler, "w1", 4).await?;
    let action = add_action(&scheduler, 1, 0, "3").await?;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(action.as_state().await?.0.stage, ActionStage::Executing);

    assert_eq!(queued_demand(scheduler.as_ref()).await?.len(), 0);

    let workers = scheduler.worker_snapshot().await;
    assert_eq!(workers.len(), 1);
    let worker = &workers[0];
    assert_eq!(worker.id, "w1");
    assert_eq!(worker.running_actions, 1);
    assert_eq!(worker.max_inflight_tasks, 4);
    assert!(!worker.is_paused && !worker.is_draining);
    assert_eq!(worker.last_update_timestamp, NOW_TIME);
    assert_eq!(worker.platform_properties["cpu_count"], "4");
    assert_eq!(worker.available_platform_properties["cpu_count"], "1");
    assert_eq!(worker.free_memory_kb, None);
    Ok(())
}

/// Both listings serialize to JSON the way the admin endpoint serves them,
/// and read back to the same values.
#[nativelink_test]
async fn listings_round_trip_through_json() -> Result<(), Error> {
    let scheduler = make_scheduler();
    let _rx = add_worker(&scheduler, "w1", 4).await?;
    let _waiting = add_action(&scheduler, 1, 2, "8").await?;
    scheduler.do_try_match_for_test().await?;

    let demand = queued_demand(scheduler.as_ref()).await?;
    let parsed: Vec<QueuedDemand> =
        serde_json::from_str(&queued_demand_json(scheduler.as_ref()).await?)
            .expect("demand JSON parses");
    assert_eq!(parsed, demand);
    assert_eq!(
        parsed.len(),
        1,
        "an 8-cpu action does not fit a 4-cpu worker"
    );

    let workers = scheduler.worker_snapshot().await;
    let parsed: Vec<WorkerSummary> =
        serde_json::from_str(&workers_json(&workers)?).expect("worker JSON parses");
    assert_eq!(parsed, workers);
    Ok(())
}

/// One listed operation as the demand listing reads it back: the filter
/// saw it queued, but each later read is fresh and may find it moved on,
/// gone, or unreadable.
#[derive(Clone)]
struct Listed {
    stage: Result<ActionStage, Code>,
}

#[async_trait]
impl ActionStateResult for Listed {
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        let stage = self
            .stage
            .clone()
            .map_err(|code| make_err!(code, "record read failed"))?;
        Ok((
            Arc::new(ActionState {
                stage,
                client_operation_id: OperationId::default(),
                action_digest: DigestInfo::zero_digest(),
                last_transition_timestamp: UNIX_EPOCH,
            }),
            None,
        ))
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        self.as_state().await
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
        Ok((
            make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest()),
            None,
        ))
    }
}

/// A scheduler whose queued listing is exactly these operations.
struct FixedListing(Vec<Listed>);

impl MetricsComponent for FixedListing {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::Component)
    }
}

#[async_trait]
impl ClientStateManager for FixedListing {
    async fn add_action(
        &self,
        _client_operation_id: OperationId,
        _action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        Err(make_err!(Code::Unimplemented, "not part of this fixture"))
    }

    async fn filter_operations(
        &self,
        _filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        let listed: Vec<Box<dyn ActionStateResult>> = self
            .0
            .iter()
            .cloned()
            .map(|listed| -> Box<dyn ActionStateResult> { Box::new(listed) })
            .collect();
        Ok(Box::pin(stream::iter(listed)))
    }
}

/// Between the filter and the read a record can be dispatched, expire or
/// turn out unreadable. Demand keeps only what is still queued, and one bad
/// record does not take the listing down with it.
#[nativelink_test]
async fn demand_leaves_out_what_moved_on_or_went_away() -> Result<(), Error> {
    let scheduler = FixedListing(vec![
        Listed {
            stage: Ok(ActionStage::Queued),
        },
        Listed {
            stage: Err(Code::NotFound),
        },
        Listed {
            stage: Ok(ActionStage::Executing),
        },
        Listed {
            stage: Err(Code::InvalidArgument),
        },
        Listed {
            stage: Ok(ActionStage::Queued),
        },
    ]);
    let demand = queued_demand(&scheduler).await?;
    assert_eq!(demand.len(), 2, "only the two still queued are demand");
    Ok(())
}

/// A read that fails because the store is unreachable is the listing's
/// error, not a record to leave out.
#[nativelink_test]
async fn demand_fails_when_the_store_does() -> Result<(), Error> {
    let scheduler = FixedListing(vec![
        Listed {
            stage: Ok(ActionStage::Queued),
        },
        Listed {
            stage: Err(Code::Unavailable),
        },
    ]);
    let err = queued_demand(&scheduler)
        .await
        .expect_err("a store failure fails the listing");
    assert_eq!(err.code, Code::Unavailable);
    Ok(())
}
