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
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::StreamExt;
use nativelink_error::Error;
use nativelink_proto::build::bazel::remote::execution::v2::{
    digest_function, ActionResult as ProtoActionResult, GetActionResultRequest,
};
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::Store;
use parking_lot::{Mutex, MutexGuard};
use scopeguard::guard;
use tokio::select;
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;
use tonic::Request;

use crate::action_scheduler::ActionScheduler;
use crate::platform_property_manager::PlatformPropertyManager;

/// Actions that are having their cache checked or failed cache lookup and are
/// being forwarded upstream.  Missing the skip_cache_check actions which are
/// forwarded directly.
type CheckActions = HashMap<ActionInfoHashKey, Arc<watch::Sender<Arc<ActionState>>>>;

pub struct CacheLookupScheduler {
    /// A reference to the CAS which is used to validate all the outputs of a
    /// cached ActionResult still exist.
    cas_store: Arc<dyn Store>,
    /// A reference to the AC to find existing actions in.
    ac_store: Arc<dyn Store>,
    /// The "real" scheduler to use to perform actions if they were not found
    /// in the action cache.
    action_scheduler: Arc<dyn ActionScheduler>,
    /// Actions that are currently performing a CacheCheck.
    cache_check_actions: Arc<Mutex<CheckActions>>,
}

async fn get_action_from_store(
    ac_store: Pin<&dyn Store>,
    action_digest: DigestInfo,
    instance_name: String,
) -> Option<ProtoActionResult> {
    // If we are a GrpcStore we shortcut here, as this is a special store.
    let any_store = ac_store.inner_store(Some(action_digest)).as_any();
    if let Some(grpc_store) = any_store.downcast_ref::<GrpcStore>() {
        let action_result_request = GetActionResultRequest {
            instance_name,
            action_digest: Some(action_digest.into()),
            inline_stdout: false,
            inline_stderr: false,
            inline_output_files: Vec::new(),
            digest_function: digest_function::Value::Sha256.into(),
        };
        grpc_store
            .get_action_result(Request::new(action_result_request))
            .await
            .map(|response| response.into_inner())
            .ok()
    } else {
        get_and_decode_digest::<ProtoActionResult>(ac_store, &action_digest)
            .await
            .ok()
    }
}

async fn validate_outputs_exist(
    cas_store: &Arc<dyn Store>,
    action_result: &ProtoActionResult,
) -> bool {
    // Verify that output_files and output_directories are available in the cas.
    let mut required_digests = Vec::with_capacity(
        action_result.output_files.len() + action_result.output_directories.len(),
    );
    for digest in action_result
        .output_files
        .iter()
        .filter_map(|output_file| output_file.digest.as_ref())
        .chain(
            action_result
                .output_directories
                .iter()
                .filter_map(|output_file| output_file.tree_digest.as_ref()),
        )
    {
        let Ok(digest) = DigestInfo::try_from(digest) else {
            return false;
        };
        required_digests.push(digest);
    }

    let Ok(sizes) = Pin::new(cas_store.as_ref())
        .has_many(&required_digests)
        .await
    else {
        return false;
    };
    sizes.into_iter().all(|size| size.is_some())
}

fn subscribe_to_existing_action(
    cache_check_actions: &MutexGuard<CheckActions>,
    unique_qualifier: &ActionInfoHashKey,
) -> Option<watch::Receiver<Arc<ActionState>>> {
    cache_check_actions.get(unique_qualifier).map(|tx| {
        let current_value = tx.borrow();
        // Subscribe marks the current value as seen, so we have to
        // re-send it to all receivers.
        // TODO: Fix this when fixed upstream tokio-rs/tokio#5871
        let rx = tx.subscribe();
        let _ = tx.send(current_value.clone());
        rx
    })
}

impl CacheLookupScheduler {
    pub fn new(
        cas_store: Arc<dyn Store>,
        ac_store: Arc<dyn Store>,
        action_scheduler: Arc<dyn ActionScheduler>,
    ) -> Result<Self, Error> {
        Ok(Self {
            cas_store,
            ac_store,
            action_scheduler,
            cache_check_actions: Default::default(),
        })
    }
}

#[async_trait]
impl ActionScheduler for CacheLookupScheduler {
    fn notify_client_disconnected(&self, _unique_qualifier: ActionInfoHashKey) {}

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
        action_info: ActionInfo,
    ) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        if action_info.skip_cache_lookup {
            // Cache lookup skipped, forward to the upstream.
            return self.action_scheduler.add_action(action_info).await;
        }
        let mut current_state = Arc::new(ActionState {
            unique_qualifier: action_info.unique_qualifier.clone(),
            stage: ActionStage::CacheCheck,
        });
        let (tx, rx) = watch::channel(current_state.clone());
        let tx = Arc::new(tx);
        let scope_guard = {
            let mut cache_check_actions = self.cache_check_actions.lock();
            // Check this isn't a duplicate request first.
            if let Some(rx) =
                subscribe_to_existing_action(&cache_check_actions, &action_info.unique_qualifier)
            {
                return Ok(rx);
            }
            cache_check_actions.insert(action_info.unique_qualifier.clone(), tx.clone());
            // In the event we loose the reference to our `scope_guard`, it will remove
            // the action from the cache_check_actions map.
            let cache_check_actions = self.cache_check_actions.clone();
            let unique_qualifier = action_info.unique_qualifier.clone();
            guard((), move |_| {
                cache_check_actions.lock().remove(&unique_qualifier);
            })
        };

        let ac_store = self.ac_store.clone();
        let cas_store = self.cas_store.clone();
        let action_scheduler = self.action_scheduler.clone();
        // We need this spawn because we are returning a stream and this spawn will populate the stream's data.
        tokio::spawn(async move {
            // If our spawn ever dies, we will remove the action from the cache_check_actions map.
            let _scope_guard = scope_guard;

            // Perform cache check.
            let action_digest = current_state.action_digest();
            let instance_name = action_info.instance_name().clone();
            if let Some(action_result) =
                get_action_from_store(Pin::new(ac_store.as_ref()), *action_digest, instance_name)
                    .await
            {
                if validate_outputs_exist(&cas_store, &action_result).await {
                    // Found in the cache, return the result immediately.
                    Arc::make_mut(&mut current_state).stage =
                        ActionStage::CompletedFromCache(action_result);
                    let _ = tx.send(current_state);
                    return;
                }
            }
            // Not in cache, forward to upstream and proxy state.
            match action_scheduler.add_action(action_info).await {
                Ok(rx) => {
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
        Ok(rx)
    }

    async fn find_existing_action(
        &self,
        unique_qualifier: &ActionInfoHashKey,
    ) -> Option<watch::Receiver<Arc<ActionState>>> {
        {
            let cache_check_actions = self.cache_check_actions.lock();
            if let Some(rx) = subscribe_to_existing_action(&cache_check_actions, unique_qualifier) {
                return Some(rx);
            }
        }
        // Cache skipped may be in the upstream scheduler.
        self.action_scheduler
            .find_existing_action(unique_qualifier)
            .await
    }

    async fn clean_recently_completed_actions(&self) {}
}
