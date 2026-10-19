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

use async_trait::async_trait;
use futures::join;
use nativelink_config::stores::MemorySpec;
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_proto::build::bazel::remote::execution::v2::ActionResult as ProtoActionResult;
use nativelink_scheduler::cache_lookup_scheduler::CacheLookupScheduler;
use nativelink_scheduler::mock_scheduler::MockActionScheduler;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::action_messages::{
    ActionResult, ActionStage, ActionState, ActionUniqueQualifier, OperationId,
};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::operation_state_manager::{ClientStateManager, OperationFilter};
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use prost::Message;
use tokio::sync::watch;
use tokio::{self};
use tokio_stream::StreamExt;
use utils::scheduler_utils::{TokioWatchActionStateResult, make_base_action_info};

/// An AC store whose reads always fail with a non-`NotFound` backend error.
#[derive(Debug, MetricsComponent)]
struct AlwaysErrStore {
    #[metric(help = "Error code every operation fails with")]
    code: u32,
}

#[async_trait]
impl StoreDriver for AlwaysErrStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        _keys: &[StoreKey<'_>],
        _results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        Err(make_err!(Code::Unavailable, "injected AC backend failure"))
    }

    async fn update(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        Err(make_err!(Code::Unavailable, "injected AC backend failure"))
    }

    async fn get_part(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        Err(make_err!(Code::Unavailable, "injected AC backend failure"))
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(
        self: Arc<Self>,
        _callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(AlwaysErrStore);

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
            std::time::Duration::from_secs(5),
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

/// A faulting AC backend (unavailable Redis, decode error) must degrade the
/// cache lookup to execution, not fail the Execute request: a cache fault
/// costs a redundant execution, never a failed build.
#[nativelink_test]
async fn ac_fault_falls_through_to_execution() -> Result<(), Error> {
    let mock_scheduler = Arc::new(MockActionScheduler::new());
    let ac_store = Store::new(Arc::new(AlwaysErrStore {
        code: Code::Unavailable as u32,
    }));
    let cache_scheduler = CacheLookupScheduler::new(ac_store, mock_scheduler.clone())?;
    let action_info = make_base_action_info(UNIX_EPOCH, DigestInfo::new([7; 32], 123));
    let (_forward_watch_channel_tx, forward_watch_channel_rx) =
        watch::channel(Arc::new(ActionState {
            client_operation_id: OperationId::default(),
            stage: ActionStage::Queued,
            action_digest: action_info.unique_qualifier.digest(),
            last_transition_timestamp: SystemTime::now(),
        }));
    let client_operation_id = OperationId::default();
    let (add_result, _) = join!(
        cache_scheduler.add_action(client_operation_id.clone(), action_info.clone()),
        mock_scheduler.expect_add_action(Ok(Box::new(TokioWatchActionStateResult::new(
            client_operation_id,
            action_info,
            forward_watch_channel_rx
        ))))
    );
    add_result.expect("AC backend fault must fall through to execution, not fail the client");
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
