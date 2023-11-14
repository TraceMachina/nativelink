// Copyright 2022 The Native Link Authors. All rights reserved.
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
use std::convert::TryInto;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use bytes::BytesMut;
use error::{make_input_err, Code, Error, ResultExt};
use native_link_config::cas_server::{AcStoreConfig, InstanceName};
use native_link_store::ac_utils::{get_and_decode_digest, ESTIMATED_DIGEST_SIZE};
use native_link_store::grpc_store::GrpcStore;
use native_link_store::store_manager::StoreManager;
use native_link_util::common::{log, DigestInfo};
use native_link_util::store_trait::Store;
use prost::Message;
use proto::build::bazel::remote::execution::v2::action_cache_server::{ActionCache, ActionCacheServer as Server};
use proto::build::bazel::remote::execution::v2::{ActionResult, GetActionResultRequest, UpdateActionResultRequest};
use tonic::{Request, Response, Status};

pub struct AcServer {
    stores: HashMap<String, Arc<dyn Store>>,
}

impl AcServer {
    pub fn new(config: &HashMap<InstanceName, AcStoreConfig>, store_manager: &StoreManager) -> Result<Self, Error> {
        let mut stores = HashMap::with_capacity(config.len());
        for (instance_name, ac_cfg) in config {
            let store = store_manager
                .get_store(&ac_cfg.ac_store)
                .ok_or_else(|| make_input_err!("'ac_store': '{}' does not exist", ac_cfg.ac_store))?;
            stores.insert(instance_name.to_string(), store);
        }
        Ok(AcServer { stores: stores.clone() })
    }

    pub fn into_service(self) -> Server<AcServer> {
        Server::new(self)
    }

    async fn inner_get_action_result(
        &self,
        grpc_request: Request<GetActionResultRequest>,
    ) -> Result<Response<ActionResult>, Error> {
        let get_action_request = grpc_request.into_inner();

        let instance_name = &get_action_request.instance_name;
        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{}'", instance_name))?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        let any_store = store.clone().as_any();
        let maybe_grpc_store = any_store.downcast_ref::<Arc<GrpcStore>>();
        if let Some(grpc_store) = maybe_grpc_store {
            return grpc_store.get_action_result(Request::new(get_action_request)).await;
        }

        // TODO(blaise.bruer) We should write a test for these errors.
        let digest: DigestInfo = get_action_request
            .action_digest
            .err_tip(|| "Action digest was not set in message")?
            .try_into()?;

        Ok(Response::new(
            get_and_decode_digest::<ActionResult>(Pin::new(store.as_ref()), &digest).await?,
        ))
    }

    async fn inner_update_action_result(
        &self,
        grpc_request: Request<UpdateActionResultRequest>,
    ) -> Result<Response<ActionResult>, Error> {
        let update_action_request = grpc_request.into_inner();

        let instance_name = &update_action_request.instance_name;
        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{}'", instance_name))?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        let any_store = store.clone().as_any();
        let maybe_grpc_store = any_store.downcast_ref::<Arc<GrpcStore>>();
        if let Some(grpc_store) = maybe_grpc_store {
            return grpc_store
                .update_action_result(Request::new(update_action_request))
                .await;
        }

        let digest: DigestInfo = update_action_request
            .action_digest
            .err_tip(|| "Action digest was not set in message")?
            .try_into()?;

        let action_result = update_action_request
            .action_result
            .err_tip(|| "Action result was not set in message")?;

        let mut store_data = BytesMut::with_capacity(ESTIMATED_DIGEST_SIZE);
        action_result
            .encode(&mut store_data)
            .err_tip(|| "Provided ActionResult could not be serialized")?;

        Pin::new(store.as_ref())
            .update_oneshot(digest, store_data.freeze())
            .await
            .err_tip(|| "Failed to update in action cache")?;
        Ok(Response::new(action_result))
    }
}

#[tonic::async_trait]
impl ActionCache for AcServer {
    async fn get_action_result(
        &self,
        grpc_request: Request<GetActionResultRequest>,
    ) -> Result<Response<ActionResult>, Status> {
        let now = Instant::now();
        log::info!("\x1b[0;31mget_action_result Req\x1b[0m: {:?}", grpc_request.get_ref());
        let hash = grpc_request
            .get_ref()
            .action_digest
            .as_ref()
            .map(|v| v.hash.to_string());
        let resp = self.inner_get_action_result(grpc_request).await;
        let d = now.elapsed().as_secs_f32();
        if resp.is_err() && resp.as_ref().err().unwrap().code != Code::NotFound {
            log::error!("\x1b[0;31mget_action_result Resp\x1b[0m: {} {:?} {:?}", d, hash, resp);
        } else {
            log::info!("\x1b[0;31mget_action_result Resp\x1b[0m: {} {:?} {:?}", d, hash, resp);
        }
        return resp.map_err(|e| e.into());
    }

    async fn update_action_result(
        &self,
        grpc_request: Request<UpdateActionResultRequest>,
    ) -> Result<Response<ActionResult>, Status> {
        let now = Instant::now();
        log::info!(
            "\x1b[0;31mupdate_action_result Req\x1b[0m: {:?}",
            grpc_request.get_ref()
        );
        let resp = self.inner_update_action_result(grpc_request).await;
        let d = now.elapsed().as_secs_f32();
        if resp.is_err() {
            log::error!("\x1b[0;31mupdate_action_result Resp\x1b[0m: {} {:?}", d, resp);
        } else {
            log::info!("\x1b[0;31mupdate_action_result Resp\x1b[0m: {} {:?}", d, resp);
        }
        return resp.map_err(|e| e.into());
    }
}
