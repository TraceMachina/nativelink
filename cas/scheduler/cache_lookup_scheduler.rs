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

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use rand::{thread_rng, Rng};
use tokio::sync::watch;
use tonic::Request;

use ac_utils::get_and_decode_digest;
use action_messages::{ActionInfo, ActionStage, ActionState};
use common::DigestInfo;
use config::schedulers::CacheLookupScheduler as SchedulerConfig;
use error::{make_input_err, Error};
use grpc_store::GrpcStore;
use platform_property_manager::PlatformPropertyManager;
use proto::build::bazel::remote::execution::v2::{
    ActionResult as ProtoActionResult, FindMissingBlobsRequest, GetActionResultRequest,
};
use scheduler::ActionScheduler;
use store::{Store, StoreManager};

pub struct CacheLookupScheduler {
    /// A reference to the CAS which is used to validate all the outputs of a
    /// cached ActionResult still exist.
    cas_store: Arc<dyn Store>,
    /// A reference to the AC to find existing actions in.
    ac_store: Arc<dyn Store>,
    /// The "real" scheduler to use to perform actions if they were not found
    /// in the action cache.
    scheduler: Arc<dyn ActionScheduler>,
}

impl CacheLookupScheduler {
    pub fn new(
        config: &SchedulerConfig,
        store_manager: &StoreManager,
        scheduler_manager: &HashMap<String, Arc<dyn ActionScheduler>>,
    ) -> Result<Self, Error> {
        let cas_store = store_manager
            .get_store(&config.cas_store)
            .ok_or_else(|| make_input_err!("'cas_store': '{}' does not exist", config.cas_store))?;
        let ac_store = store_manager
            .get_store(&config.ac_store)
            .ok_or_else(|| make_input_err!("'ac_store': '{}' does not exist", config.ac_store))?;
        let scheduler = scheduler_manager
            .get(&config.scheduler)
            .ok_or_else(|| make_input_err!("'scheduler': '{}' does not exist", config.scheduler))?
            .clone();
        Ok(Self {
            cas_store,
            ac_store,
            scheduler,
        })
    }

    async fn get_action_from_store(&self, action_digest: &DigestInfo) -> Option<ProtoActionResult> {
        // If we are a GrpcStore we shortcut here, as this is a special store.
        let any_store = self.ac_store.clone().as_any();
        let maybe_grpc_store = any_store.downcast_ref::<Arc<GrpcStore>>();
        let result = if let Some(grpc_store) = maybe_grpc_store {
            let action_result_request = GetActionResultRequest {
                instance_name: "".to_string(),
                action_digest: Some(action_digest.into()),
                inline_stdout: false,
                inline_stderr: false,
                inline_output_files: Vec::new(),
            };
            grpc_store
                .get_action_result(Request::new(action_result_request))
                .await
                .map(|response| response.into_inner())
        } else {
            get_and_decode_digest::<ProtoActionResult>(Pin::new(self.ac_store.as_ref()), &action_digest).await
        };
        result.ok()
    }

    async fn validate_outputs_exist(&self, action_result: &ProtoActionResult) -> bool {
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
        let any_store = self.cas_store.clone().as_any();
        let maybe_grpc_store = any_store.downcast_ref::<Arc<GrpcStore>>();
        if let Some(grpc_store) = maybe_grpc_store {
            let find_result = grpc_store
                .find_missing_blobs(Request::new(FindMissingBlobsRequest {
                    instance_name: "".to_string(),
                    blob_digests: required_digests,
                }))
                .await;
            if find_result.is_err() || !find_result.unwrap().into_inner().missing_blob_digests.is_empty() {
                return false;
            }
        } else {
            let cas_pin = Pin::new(self.cas_store.as_ref());
            for digest in required_digests
                .iter()
                .filter_map(|digest| DigestInfo::try_from(digest.clone()).ok())
            {
                if cas_pin.has(digest).await.is_err() {
                    return false;
                }
            }
        };
        true
    }
}

#[async_trait]
impl ActionScheduler for CacheLookupScheduler {
    fn get_platform_property_manager(&self) -> &PlatformPropertyManager {
        self.scheduler.get_platform_property_manager()
    }

    async fn add_action(&self, action_info: ActionInfo) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        if !action_info.skip_cache_lookup {
            let action_digest = action_info.digest();
            if let Some(proto_action_result) = self.get_action_from_store(action_digest).await {
                if self.validate_outputs_exist(&proto_action_result).await {
                    // Found in the cache, return the result immediately.
                    let current_state = Arc::new(ActionState {
                        name: format!("{:X}", thread_rng().gen::<u128>()),
                        stage: ActionStage::CompletedFromCache(proto_action_result),
                        action_digest: action_digest.clone(),
                    });
                    return Ok(watch::channel(current_state).1);
                }
            }
        }
        // Not in cache or invalid, forward to real scheduler.
        self.scheduler.add_action(action_info).await
    }
}
