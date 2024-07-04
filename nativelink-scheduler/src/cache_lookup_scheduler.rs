// Copyright 2023 The NativeLink Authors. All rights reserved.
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
use futures::future::Shared as SharedFuture;
use futures::stream::StreamExt;
use futures::FutureExt;
use nativelink_error::{make_err, Code, Error};
use nativelink_proto::build::bazel::remote::execution::v2::{
    ActionResult as ProtoActionResult, GetActionResultRequest,
};
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState, ClientOperationId,
    OperationId,
};
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::store_trait::{Store, StoreLike};
use parking_lot::{Mutex, MutexGuard};
use scopeguard::guard;
use tokio::select;
use tokio::sync::{oneshot, watch};
use tokio_stream::wrappers::WatchStream;
use tonic::Request;
use tracing::{event, Level};

use crate::action_scheduler::ActionScheduler;
use crate::platform_property_manager::PlatformPropertyManager;

/// Actions that are having their cache checked or failed cache lookup and are
/// being forwarded upstream.  Missing the skip_cache_check actions which are
/// forwarded directly.
type CheckActions = HashMap<
    ActionInfoHashKey,
    (
        SharedFuture<oneshot::Receiver<ClientOperationId>>,
        watch::Receiver<Arc<ActionState>>,
    ),
>;

pub struct CacheLookupScheduler {
    /// A reference to the AC to find existing actions in.
    /// To prevent unintended issues, this store should probably be a CompletenessCheckingStore.
    ac_store: Store,
    /// The "real" scheduler to use to perform actions if they were not found
    /// in the action cache.
    action_scheduler: Arc<dyn ActionScheduler>,
    /// Actions that are currently performing a CacheCheck.
    cache_check_actions: Arc<Mutex<CheckActions>>,
}

async fn get_action_from_store(
    ac_store: &Store,
    action_digest: DigestInfo,
    instance_name: String,
    digest_function: DigestHasherFunc,
) -> Option<ProtoActionResult> {
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
            .ok()
    } else {
        get_and_decode_digest::<ProtoActionResult>(ac_store, action_digest.into())
            .await
            .ok()
    }
}

fn subscribe_to_existing_action(
    cache_check_actions: &MutexGuard<CheckActions>,
    unique_qualifier: &ActionInfoHashKey,
) -> Option<(
    SharedFuture<oneshot::Receiver<ClientOperationId>>,
    watch::Receiver<Arc<ActionState>>,
)> {
    cache_check_actions
        .get(unique_qualifier)
        .map(|(client_operation_id_rx, rx)| {
            let mut rx = rx.clone();
            rx.mark_changed();
            (client_operation_id_rx.clone(), rx)
        })
}

impl CacheLookupScheduler {
    pub fn new(ac_store: Store, action_scheduler: Arc<dyn ActionScheduler>) -> Result<Self, Error> {
        Ok(Self {
            ac_store,
            action_scheduler,
            cache_check_actions: Default::default(),
        })
    }
}

#[async_trait]
impl ActionScheduler for CacheLookupScheduler {
    async fn get_platform_property_manager(
        &self,
        instance_name: &str,
    ) -> Result<Arc<PlatformPropertyManager>, Error> {
        self.action_scheduler
            .get_platform_property_manager(instance_name)
            .await
    }

    async fn add_action(
        &self,
        client_operation_id: ClientOperationId,
        action_info: ActionInfo,
    ) -> Result<(ClientOperationId, watch::Receiver<Arc<ActionState>>), Error> {
        if action_info.skip_cache_lookup {
            // Cache lookup skipped, forward to the upstream.
            return self
                .action_scheduler
                .add_action(client_operation_id, action_info)
                .await;
        }
        let mut current_state = Arc::new(ActionState {
            id: OperationId::new(action_info.unique_qualifier.clone()),
            stage: ActionStage::CacheCheck,
        });
        let cache_check_result = {
            // Check this isn't a duplicate request first.
            let mut cache_check_actions = self.cache_check_actions.lock();
            let current_state = current_state.clone();
            let unique_qualifier = action_info.unique_qualifier.clone();
            subscribe_to_existing_action(&cache_check_actions, &unique_qualifier).ok_or_else(
                move || {
                    let (client_operation_id_tx, client_operation_id_rx) = oneshot::channel();
                    let client_operation_id_rx = client_operation_id_rx.shared();
                    let (tx, rx) = watch::channel(current_state);
                    cache_check_actions.insert(
                        unique_qualifier.clone(),
                        (client_operation_id_rx.clone(), rx),
                    );
                    // In the event we loose the reference to our `scope_guard`, it will remove
                    // the action from the cache_check_actions map.
                    let cache_check_actions = self.cache_check_actions.clone();
                    (
                        client_operation_id_tx,
                        client_operation_id_rx,
                        tx,
                        guard((), move |_| {
                            cache_check_actions.lock().remove(&unique_qualifier);
                        }),
                    )
                },
            )
        };
        let (client_operation_id_tx, client_operation_id_rx, tx, scope_guard) =
            match cache_check_result {
                Ok((client_operation_id_tx, rx)) => {
                    let client_operation_id = client_operation_id_tx.await.map_err(|_| {
                        make_err!(
                            Code::Internal,
                            "Client operation id tx hung up in CacheLookupScheduler::add_action"
                        )
                    })?;
                    return Ok((client_operation_id, rx));
                }
                Err(client_tx_and_scope_guard) => client_tx_and_scope_guard,
            };
        let rx = tx.subscribe();

        let ac_store = self.ac_store.clone();
        let action_scheduler = self.action_scheduler.clone();
        let client_operation_id_clone = client_operation_id.clone();
        // We need this spawn because we are returning a stream and this spawn will populate the stream's data.
        background_spawn!("cache_lookup_scheduler_add_action", async move {
            // If our spawn ever dies, we will remove the action from the cache_check_actions map.
            let _scope_guard = scope_guard;

            // Perform cache check.
            let action_digest = current_state.action_digest();
            let instance_name = action_info.instance_name().clone();
            if let Some(action_result) = get_action_from_store(
                &ac_store,
                *action_digest,
                instance_name,
                current_state.id.unique_qualifier.digest_function,
            )
            .await
            {
                match ac_store.has(*action_digest).await {
                    Ok(Some(_)) => {
                        Arc::make_mut(&mut current_state).stage =
                            ActionStage::CompletedFromCache(action_result);
                        let _ = tx.send(current_state);
                        return;
                    }
                    Err(err) => {
                        event!(
                            Level::WARN,
                            ?err,
                            "Error while calling `has` on `ac_store` in `CacheLookupScheduler`'s `add_action` function"
                        );
                    }
                    _ => {}
                }
            }
            // Not in cache, forward to upstream and proxy state.
            match action_scheduler
                .add_action(client_operation_id_clone, action_info)
                .await
            {
                Ok((new_client_operation_id, rx)) => {
                    // It's ok if the other end hung up, just keep going just
                    // in case they come back.
                    let _ = client_operation_id_tx.send(new_client_operation_id);
                    let mut watch_stream = WatchStream::new(rx);
                    loop {
                        select!(
                            Some(action_state) = watch_stream.next() => {
                                if tx.send(action_state).is_err() {
                                    break;
                                }
                            }
                            _ = tx.closed() => {
                                break;
                            }
                        )
                    }
                }
                Err(err) => {
                    Arc::make_mut(&mut current_state).stage =
                        ActionStage::Completed(ActionResult {
                            error: Some(err),
                            ..Default::default()
                        });
                    let _ = tx.send(current_state);
                }
            }
        });
        let client_operation_id = client_operation_id_rx.await.map_err(|_| {
            make_err!(
                Code::Internal,
                "Client operation id tx hung up in CacheLookupScheduler::add_action"
            )
        })?;
        Ok((client_operation_id, rx))
    }

    async fn find_by_client_operation_id(
        &self,
        client_operation_id: &ClientOperationId,
    ) -> Result<Option<watch::Receiver<Arc<ActionState>>>, Error> {
        self.action_scheduler
            .find_by_client_operation_id(client_operation_id)
            .await
    }

    async fn clean_recently_completed_actions(&self) {}
}
