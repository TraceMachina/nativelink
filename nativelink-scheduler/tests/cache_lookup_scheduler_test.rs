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
use std::time::{SystemTime, UNIX_EPOCH};

mod utils {
    pub(crate) mod scheduler_utils;
}

use core::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::join;
use nativelink_config::stores::MemorySpec;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_proto::build::bazel::remote::execution::v2::ActionResult as ProtoActionResult;
use nativelink_scheduler::cache_lookup_scheduler::CacheLookupScheduler;
use nativelink_scheduler::mock_scheduler::MockActionScheduler;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionStage, ActionState, ActionUniqueQualifier, OperationId,
};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::operation_state_manager::{ClientStateManager, OperationFilter};
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use prost::Message;
use tokio::sync::{Semaphore, watch};
use tokio::{self};
use tokio_stream::StreamExt;
use utils::scheduler_utils::{TokioWatchActionStateResult, make_base_action_info};

struct TestContext {
    mock_scheduler: Arc<MockActionScheduler>,
    ac_store: Store,
    cache_scheduler: CacheLookupScheduler,
}

fn make_cache_scheduler() -> Result<TestContext, Error> {
    let mock_scheduler = Arc::new(MockActionScheduler::new());
    let ac_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let cache_scheduler = CacheLookupScheduler::new(ac_store.clone(), mock_scheduler.clone())?;
    Ok(TestContext {
        mock_scheduler,
        ac_store,
        cache_scheduler,
    })
}

#[nativelink_test]
async fn different_invocations_share_completed_cache_results() -> Result<(), Error> {
    let context = make_cache_scheduler()?;
    let action_info = make_base_action_info(UNIX_EPOCH, DigestInfo::new([42; 32], 123));
    let result = ProtoActionResult::try_from(ActionResult::default())?;
    context
        .ac_store
        .update_oneshot(action_info.digest(), result.encode_to_vec().into())
        .await?;
    for scope in ["invocation-one", "invocation-two"] {
        let mut scoped_action = action_info.clone();
        let ActionUniqueQualifier::Cacheable(key) =
            &mut Arc::make_mut(&mut scoped_action).unique_qualifier
        else {
            panic!("Expected cacheable action");
        };
        key.execution_scope = Some(scope.to_string());
        // No mock scheduler response: a regression that rekeys the cache would
        // delegate to execution and fail this bounded wait.
        let listener = tokio::time::timeout(
            Duration::from_secs(5),
            context
                .cache_scheduler
                .add_action(OperationId::default(), scoped_action),
        )
        .await
        .expect("Completed cache entry must be reusable across invocations")?;
        let (state, _) = listener.as_state().await?;
        assert_eq!(state.action_digest, action_info.digest());
        assert_eq!(state.stage, ActionStage::CompletedFromCache(result.clone()));
    }
    Ok(())
}

#[nativelink_test]
async fn add_action_handles_skip_cache() -> Result<(), Error> {
    let context = make_cache_scheduler()?;
    let action_info = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());
    let action_result = ProtoActionResult::try_from(ActionResult::default())?;
    context
        .ac_store
        .update_oneshot(action_info.digest(), action_result.encode_to_vec().into())
        .await?;
    let (_forward_watch_channel_tx, forward_watch_channel_rx) =
        watch::channel(Arc::new(ActionState {
            client_operation_id: OperationId::default(),
            stage: ActionStage::Queued,
            action_digest: action_info.unique_qualifier.digest(),
            last_transition_timestamp: SystemTime::now(),
        }));
    let ActionUniqueQualifier::Cacheable(action_key) = action_info.unique_qualifier.clone() else {
        panic!("This test should be testing when item was cached first");
    };
    let mut skip_cache_action = action_info.as_ref().clone();
    skip_cache_action.unique_qualifier = ActionUniqueQualifier::Uncacheable(action_key);
    let skip_cache_action = Arc::new(skip_cache_action);
    let client_operation_id = OperationId::default();
    let _unused = join!(
        context
            .cache_scheduler
            .add_action(client_operation_id.clone(), skip_cache_action),
        context
            .mock_scheduler
            .expect_add_action(Ok(Box::new(TokioWatchActionStateResult::new(
                client_operation_id,
                action_info,
                forward_watch_channel_rx
            ))))
    );
    Ok(())
}

#[nativelink_test]
async fn find_by_client_operation_id_call_passed() -> Result<(), Error> {
    let context = make_cache_scheduler()?;
    let client_operation_id = OperationId::default();
    let (actual_result, actual_filter) = join!(
        context.cache_scheduler.filter_operations(OperationFilter {
            client_operation_id: Some(client_operation_id.clone()),
            ..Default::default()
        }),
        context
            .mock_scheduler
            .expect_filter_operations(Ok(Box::pin(futures::stream::empty()))),
    );
    assert_eq!(true, actual_result.unwrap().next().await.is_none());
    assert_eq!(
        OperationFilter {
            client_operation_id: Some(client_operation_id),
            ..Default::default()
        },
        actual_filter
    );
    Ok(())
}

/// A `Store` that parks every `get_part` (the cache-lookup read) on a gate until
/// the test releases it, signalling on `entered_tx` when each call arrives.  This
/// gives a deterministic seam: the test can hold a request inside its cache check
/// (so its in-flight map entry is still present) while another request completes.
#[derive(MetricsComponent)]
struct PausableAcStore {
    inner: Store,
    entered_tx: tokio::sync::mpsc::UnboundedSender<()>,
    gate: Arc<Semaphore>,
}

#[async_trait]
impl StoreDriver for PausableAcStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.inner.has_with_results(keys, results).await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        self.inner.update(key, reader, size_info).await
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        // Announce arrival, then block until the test grants a permit.
        let _ = self.entered_tx.send(());
        self.gate
            .acquire()
            .await
            .expect("gate semaphore closed")
            .forget();
        self.inner.get_part(key, writer, offset, length).await
    }

    fn register_remove_callback(self: Arc<Self>, _callback: RemoveCallback) -> Result<(), Error> {
        Ok(())
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &'_ dyn StoreDriver {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }
}

default_health_status_indicator!(PausableAcStore);

fn queued_state_result(
    action_info: &Arc<ActionInfo>,
) -> Box<dyn nativelink_util::operation_state_manager::ActionStateResult> {
    let (_tx, rx) = watch::channel(Arc::new(ActionState {
        client_operation_id: OperationId::default(),
        stage: ActionStage::Queued,
        action_digest: action_info.unique_qualifier.digest(),
        last_transition_timestamp: SystemTime::now(),
    }));
    Box::new(TokioWatchActionStateResult::new(
        OperationId::default(),
        action_info.clone(),
        rx,
    ))
}

/// Regression for the cache-lookup drop-guard clobber (N8).
///
/// The leader for a cacheable key removes its own `inflight_cache_checks` entry
/// when its cache check resolves, but the spawn's drop-guard *also* removes the
/// key when the spawn exits.  A second request for the same key that inserts a
/// fresh entry in the window between the leader's remove and the guard drop has
/// its entry clobbered by the stale guard -> its oneshot sender is dropped ->
/// the client sees a spurious `tx hung up` error and must retry.
///
/// Deterministic seam: `PausableAcStore` holds the second request inside its
/// cache check (entry still in the map) while the leader completes and its guard
/// fires.  Without the `ScopeGuard::into_inner` defuse, the second request is
/// orphaned; with it, the second request survives and completes.
#[nativelink_test]
async fn concurrent_same_key_survives_completed_leader_guard() -> Result<(), Error> {
    let mock_scheduler = Arc::new(MockActionScheduler::new());
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(Semaphore::new(0));
    let ac_store = Store::new(Arc::new(PausableAcStore {
        inner,
        entered_tx,
        gate: gate.clone(),
    }));
    let cache_scheduler = Arc::new(CacheLookupScheduler::new(ac_store, mock_scheduler.clone())?);

    // Empty AC store -> every cache check misses (NotFound) -> leader delegates
    // to the action scheduler, which is where we park it.
    let action_info = make_base_action_info(UNIX_EPOCH, DigestInfo::new([7; 32], 1));

    // Request 1 becomes the leader.
    let cs1 = cache_scheduler.clone();
    let ai1 = action_info.clone();
    let r1 = tokio::spawn(async move { cs1.add_action(OperationId::default(), ai1).await });

    // R1 reaches its cache check; release it.  It misses, removes its own
    // in-flight entry, and parks delegating to the action scheduler.
    entered_rx
        .recv()
        .await
        .expect("R1 should reach cache check");
    gate.add_permits(1);
    // Rendezvous proves R1 passed its self-remove and is parked (do not respond yet).
    let _r1_delegated = mock_scheduler.receive_add_action().await;

    // Request 2 for the SAME key: R1's entry is gone, so R2 becomes a fresh
    // leader, inserts a new entry, and parks in its own cache check.
    let cs2 = cache_scheduler.clone();
    let ai2 = action_info.clone();
    let r2 = tokio::spawn(async move { cs2.add_action(OperationId::default(), ai2).await });
    entered_rx
        .recv()
        .await
        .expect("R2 should reach cache check");

    // Complete R1 -> its spawn exits -> its drop-guard fires.  Without the fix
    // this removes R2's fresh entry (the clobber).
    mock_scheduler.respond_add_action(Ok(queued_state_result(&action_info)));
    r1.await
        .expect("R1 task should join")
        .expect("R1 should succeed");

    // Release R2.  With the fix its entry survived, so it delegates to the
    // action scheduler; without the fix it was orphaned and never delegates.
    gate.add_permits(1);
    if tokio::time::timeout(Duration::from_secs(2), mock_scheduler.receive_add_action())
        .await
        .is_ok()
    {
        mock_scheduler.respond_add_action(Ok(queued_state_result(&action_info)));
    }

    let r2_result = tokio::time::timeout(Duration::from_secs(10), r2)
        .await
        .expect("R2 task should finish")
        .expect("R2 task should join");

    // The concurrent same-key request must NOT be orphaned by the completed
    // leader's stale drop-guard.
    assert!(
        r2_result.is_ok(),
        "concurrent same-key request was clobbered by the completed leader's drop-guard: {:?}",
        r2_result.err()
    );
    Ok(())
}
