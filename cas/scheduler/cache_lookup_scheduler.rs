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

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;
use tonic::Request;

use ac_utils::get_and_decode_digest;
use action_messages::{ActionInfo, ActionResult, ActionStage, ActionState};
use common::DigestInfo;
use error::Error;
use grpc_store::GrpcStore;
use platform_property_manager::PlatformPropertyManager;
use proto::build::bazel::remote::execution::v2::{
    ActionResult as ProtoActionResult, FindMissingBlobsRequest, GetActionResultRequest,
};
use scheduler::ActionScheduler;
use store::Store;

pub struct CacheLookupScheduler {
    /// A reference to the CAS which is used to validate all the outputs of a
    /// cached ActionResult still exist.
    cas_store: Arc<dyn Store>,
    /// A reference to the AC to find existing actions in.
    ac_store: Arc<dyn Store>,
    /// The "real" scheduler to use to perform actions if they were not found
    /// in the action cache.
    action_scheduler: Arc<dyn ActionScheduler>,
}

async fn get_action_from_store(
    ac_store: Arc<dyn Store>,
    action_digest: &DigestInfo,
    instance_name: String,
) -> Option<ProtoActionResult> {
    // If we are a GrpcStore we shortcut here, as this is a special store.
    let any_store = ac_store.clone().as_any();
    let maybe_grpc_store = any_store.downcast_ref::<Arc<GrpcStore>>();
    if let Some(grpc_store) = maybe_grpc_store {
        let action_result_request = GetActionResultRequest {
            instance_name,
            action_digest: Some(action_digest.into()),
            inline_stdout: false,
            inline_stderr: false,
            inline_output_files: Vec::new(),
        };
        grpc_store
            .get_action_result(Request::new(action_result_request))
            .await
            .map(|response| response.into_inner())
            .ok()
    } else {
        get_and_decode_digest::<ProtoActionResult>(Pin::new(ac_store.as_ref()), action_digest)
            .await
            .ok()
    }
}

async fn validate_outputs_exist(
    cas_store: Arc<dyn Store>,
    action_result: &ProtoActionResult,
    instance_name: String,
) -> bool {
    // Verify that output_files and output_directories are available in the cas.
    let required_digests = action_result
        .output_files
        .iter()
        .filter_map(|output_file| output_file.digest.clone())
        .chain(
            action_result
                .output_directories
                .iter()
                .filter_map(|output_directory| output_directory.tree_digest.clone()),
        )
        .collect();

    // If the CAS is a GrpcStore store we can check all the digests in one message.
    let any_store = cas_store.clone().as_any();
    let maybe_grpc_store = any_store.downcast_ref::<Arc<GrpcStore>>();
    if let Some(grpc_store) = maybe_grpc_store {
        grpc_store
            .find_missing_blobs(Request::new(FindMissingBlobsRequest {
                instance_name,
                blob_digests: required_digests,
            }))
            .await
            .is_ok_and(|find_result| find_result.into_inner().missing_blob_digests.is_empty())
    } else {
        let cas_pin = Pin::new(cas_store.as_ref());
        required_digests
            .into_iter()
            .map(|digest| async move { cas_pin.has(DigestInfo::try_from(digest)?).await })
            .collect::<FuturesUnordered<_>>()
            .all(|result| async { result.is_ok_and(|result| result.is_some()) })
            .await
    }
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
        })
    }
}

#[async_trait]
impl ActionScheduler for CacheLookupScheduler {
    async fn get_platform_property_manager(&self, instance_name: &str) -> Result<Arc<PlatformPropertyManager>, Error> {
        self.action_scheduler.get_platform_property_manager(instance_name).await
    }

    async fn add_action(&self, action_info: ActionInfo) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        if action_info.skip_cache_lookup {
            // Cache lookup skipped, forward to the upstream.
            return self.action_scheduler.add_action(action_info).await;
        }
        let mut current_state = Arc::new(ActionState {
            unique_qualifier: action_info.unique_qualifier.clone(),
            stage: ActionStage::CacheCheck,
        });
        let (tx, rx) = watch::channel(current_state.clone());
        let ac_store = self.ac_store.clone();
        let cas_store = self.cas_store.clone();
        let action_scheduler = self.action_scheduler.clone();
        tokio::spawn(async move {
            let instance_name = action_info.instance_name().clone();
            if let Some(proto_action_result) =
                get_action_from_store(ac_store, current_state.action_digest(), instance_name.clone()).await
            {
                if validate_outputs_exist(cas_store, &proto_action_result, instance_name).await {
                    // Found in the cache, return the result immediately.
                    Arc::make_mut(&mut current_state).stage = ActionStage::CompletedFromCache(proto_action_result);
                    let _ = tx.send(current_state);
                    return;
                }
            }
            // Not in cache, forward to upstream and proxy state.
            let mut watch_stream = match action_scheduler.add_action(action_info).await {
                Ok(rx) => WatchStream::new(rx),
                Err(err) => {
                    Arc::make_mut(&mut current_state).stage = ActionStage::Error((err, ActionResult::default()));
                    let _ = tx.send(current_state);
                    return;
                }
            };
            while let Some(action_state) = watch_stream.next().await {
                if tx.send(action_state).is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}
