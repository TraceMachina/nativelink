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

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_proto::build::bazel::remote::execution::v2::{
    ActionResult as ProtoActionResult, GetActionResultRequest,
};
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionState, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
};
use nativelink_util::store_trait::Store;
use parking_lot::{Mutex, MutexGuard};
use scopeguard::guard;
use tokio::sync::oneshot;
use tonic::Request;
use tracing::{event, Level};

/// Actions that are having their cache checked or failed cache lookup and are
/// being forwarded upstream.  Missing the skip_cache_check actions which are
/// forwarded directly.
type CheckActions = HashMap<
    ActionUniqueKey,
    Vec<(
        OperationId,
        oneshot::Sender<Result<Box<dyn ActionStateResult>, Error>>,
    )>,
>;

#[derive(MetricsComponent)]
pub struct CacheLookupScheduler {
    /// A reference to the AC to find existing actions in.
    /// To prevent unintended issues, this store should probably be a CompletenessCheckingStore.
    #[metric(group = "ac_store")]
    ac_store: Store,
    /// The "real" scheduler to use to perform actions if they were not found
    /// in the action cache.
    #[metric(group = "action_scheduler")]
    action_scheduler: Arc<dyn ClientStateManager>,
    /// Actions that are currently performing a CacheCheck.
    inflight_cache_checks: Arc<Mutex<CheckActions>>,
}

async fn get_action_from_store(
    ac_store: &Store,
    action_digest: DigestInfo,
    instance_name: String,
    digest_function: DigestHasherFunc,
) -> Result<ProtoActionResult, Error> {
    // If we are a GrpcStore we shortcut here, as this is a special store.
    if let Some(grpc_store) = ac_store.downcast_ref::<GrpcStore>(Some(action_digest.into())) {
        let action_result_request = GetActionResultRequest {
            instance_name,
            action_digest: Some(action_digest.into()),
            inline_stdout: false,
            inline_stderr: false,
            inline_output_files: Vec::new(),
            digest_function: digest_function.proto_digest_func().into(),
        };
        grpc_store
            .get_action_result(Request::new(action_result_request))
            .await
            .map(|response| response.into_inner())
    } else {
        get_and_decode_digest::<ProtoActionResult>(ac_store, action_digest.into()).await
    }
}

/// Future for when ActionStateResults are known.
type ActionStateResultOneshot = oneshot::Receiver<Result<Box<dyn ActionStateResult>, Error>>;

fn subscribe_to_existing_action(
    inflight_cache_checks: &mut MutexGuard<CheckActions>,
    unique_qualifier: &ActionUniqueKey,
    client_operation_id: &OperationId,
) -> Option<ActionStateResultOneshot> {
    inflight_cache_checks
        .get_mut(unique_qualifier)
        .map(|oneshots| {
            let (tx, rx) = oneshot::channel();
            oneshots.push((client_operation_id.clone(), tx));
            rx
        })
}

struct CacheLookupActionStateResult {
    action_state: Arc<ActionState>,
    change_called: bool,
}

#[async_trait]
impl ActionStateResult for CacheLookupActionStateResult {
    async fn as_state(&self) -> Result<Arc<ActionState>, Error> {
        Ok(self.action_state.clone())
    }

    async fn changed(&mut self) -> Result<Arc<ActionState>, Error> {
        if self.change_called {
            return Err(make_err!(
                Code::Internal,
                "CacheLookupActionStateResult::changed called twice"
            ));
        }
        self.change_called = true;
        Ok(self.action_state.clone())
    }

    async fn as_action_info(&self) -> Result<Arc<ActionInfo>, Error> {
        // TODO(allada) We should probably remove as_action_info()
        // or implement it properly.
        return Err(make_err!(
            Code::Unimplemented,
            "as_action_info not implemented for CacheLookupActionStateResult::as_action_info"
        ));
    }
}

impl CacheLookupScheduler {
    pub fn new(
        ac_store: Store,
        action_scheduler: Arc<dyn ClientStateManager>,
    ) -> Result<Self, Error> {
        Ok(Self {
            ac_store,
            action_scheduler,
            inflight_cache_checks: Default::default(),
        })
    }

    async fn inner_add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        let unique_key = match &action_info.unique_qualifier {
            ActionUniqueQualifier::Cachable(unique_key) => unique_key.clone(),
            ActionUniqueQualifier::Uncachable(_) => {
                // Cache lookup skipped, forward to the upstream.
                return self
                    .action_scheduler
                    .add_action(client_operation_id, action_info)
                    .await;
            }
        };

        let cache_check_result = {
            // Check this isn't a duplicate request first.
            let mut inflight_cache_checks = self.inflight_cache_checks.lock();
            subscribe_to_existing_action(
                &mut inflight_cache_checks,
                &unique_key,
                &client_operation_id,
            )
            .ok_or_else(move || {
                let (action_listener_tx, action_listener_rx) = oneshot::channel();
                inflight_cache_checks.insert(
                    unique_key.clone(),
                    vec![(client_operation_id, action_listener_tx)],
                );
                // In the event we loose the reference to our `scope_guard`, it will remove
                // the action from the inflight_cache_checks map.
                let inflight_cache_checks = self.inflight_cache_checks.clone();
                (
                    action_listener_rx,
                    guard((), move |_| {
                        inflight_cache_checks.lock().remove(&unique_key);
                    }),
                )
            })
        };
        let (action_listener_rx, scope_guard) = match cache_check_result {
            Ok(action_listener_fut) => {
                let action_listener = action_listener_fut.await.map_err(|_| {
                    make_err!(
                        Code::Internal,
                        "ActionStateResult tx hung up in CacheLookupScheduler::add_action"
                    )
                })?;
                return action_listener;
            }
            Err(client_tx_and_scope_guard) => client_tx_and_scope_guard,
        };

        let ac_store = self.ac_store.clone();
        let action_scheduler = self.action_scheduler.clone();
        let inflight_cache_checks = self.inflight_cache_checks.clone();
        // We need this spawn because we are returning a stream and this spawn will populate the stream's data.
        background_spawn!("cache_lookup_scheduler_add_action", async move {
            // If our spawn ever dies, we will remove the action from the inflight_cache_checks map.
            let _scope_guard = scope_guard;

            let unique_key = match &action_info.unique_qualifier {
                ActionUniqueQualifier::Cachable(unique_key) => unique_key,
                ActionUniqueQualifier::Uncachable(unique_key) => {
                    event!(
                        Level::ERROR,
                        ?action_info,
                        "ActionInfo::unique_qualifier should be ActionUniqueQualifier::Cachable()"
                    );
                    unique_key
                }
            };

            // Perform cache check.
            let instance_name = action_info.unique_qualifier.instance_name().clone();
            let maybe_action_result = get_action_from_store(
                &ac_store,
                action_info.unique_qualifier.digest(),
                instance_name,
                action_info.unique_qualifier.digest_function(),
            )
            .await;
            match maybe_action_result {
                Ok(action_result) => {
                    let maybe_pending_txs = {
                        let mut inflight_cache_checks = inflight_cache_checks.lock();
                        // We are ready to resolve the in-flight actions. We remove the
                        // in-flight actions from the map.
                        inflight_cache_checks.remove(unique_key)
                    };
                    let Some(pending_txs) = maybe_pending_txs else {
                        return; // Nobody is waiting for this action anymore.
                    };
                    let mut action_state = ActionState {
                        client_operation_id: OperationId::default(),
                        stage: ActionStage::CompletedFromCache(action_result),
                        action_digest: action_info.unique_qualifier.digest(),
                    };

                    for (client_operation_id, pending_tx) in pending_txs {
                        action_state.client_operation_id = client_operation_id;
                        // Ignore errors here, as the other end may have hung up.
                        let _ = pending_tx.send(Ok(Box::new(CacheLookupActionStateResult {
                            action_state: Arc::new(action_state.clone()),
                            change_called: false,
                        })));
                    }
                    return;
                }
                Err(err) => {
                    // NotFound errors just mean we need to execute our action.
                    if err.code != Code::NotFound {
                        let err = err.append("In CacheLookupScheduler::add_action");
                        let maybe_pending_txs = {
                            let mut inflight_cache_checks = inflight_cache_checks.lock();
                            // We are ready to resolve the in-flight actions. We remove the
                            // in-flight actions from the map.
                            inflight_cache_checks.remove(unique_key)
                        };
                        let Some(pending_txs) = maybe_pending_txs else {
                            return; // Nobody is waiting for this action anymore.
                        };
                        for (_client_operation_id, pending_tx) in pending_txs {
                            // Ignore errors here, as the other end may have hung up.
                            let _ = pending_tx.send(Err(err.clone()));
                        }
                        return;
                    }
                }
            }

            let maybe_pending_txs = {
                let mut inflight_cache_checks = inflight_cache_checks.lock();
                inflight_cache_checks.remove(unique_key)
            };
            let Some(pending_txs) = maybe_pending_txs else {
                return; // Noone is waiting for this action anymore.
            };

            for (client_operation_id, pending_tx) in pending_txs {
                // Ignore errors here, as the other end may have hung up.
                let _ = pending_tx.send(
                    action_scheduler
                        .add_action(client_operation_id, action_info.clone())
                        .await,
                );
            }
        });
        action_listener_rx
            .await
            .map_err(|_| {
                make_err!(
                    Code::Internal,
                    "ActionStateResult tx hung up in CacheLookupScheduler::add_action"
                )
            })?
            .err_tip(|| "In CacheLookupScheduler::add_action")
    }

    async fn inner_filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        self.action_scheduler
            .filter_operations(filter)
            .await
            .err_tip(|| "In CacheLookupScheduler::filter_operations")
    }
}

#[async_trait]
impl ClientStateManager for CacheLookupScheduler {
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        self.inner_add_action(client_operation_id, action_info)
            .await
    }

    async fn filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        self.inner_filter_operations(filter).await
    }

    fn as_known_platform_property_provider(&self) -> Option<&dyn KnownPlatformPropertyProvider> {
        self.action_scheduler.as_known_platform_property_provider()
    }
}

impl RootMetricsComponent for CacheLookupScheduler {}
