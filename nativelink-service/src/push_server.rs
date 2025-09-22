// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use core::convert::Into;
use std::collections::HashMap;

use nativelink_config::cas_server::{PushConfig, WithInstanceName};
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_proto::build::bazel::remote::asset::v1::push_server::{Push, PushServer as Server};
use nativelink_proto::build::bazel::remote::asset::v1::{
    PushBlobRequest, PushBlobResponse, PushDirectoryRequest, PushDirectoryResponse,
};
use nativelink_store::store_manager::StoreManager;
use nativelink_util::digest_hasher::make_ctx_for_hash_func;
use nativelink_util::store_trait::{Store, StoreLike};
use opentelemetry::context::FutureExt;
use tonic::{Request, Response, Status};
use tracing::{Instrument, Level, error_span, info, instrument};

use crate::remote_asset_proto::RemoteAssetArtifact;

#[derive(Debug, Clone)]
pub struct PushStoreInfo {
    store: Store,
    read_only: bool,
}

#[derive(Debug, Clone)]
pub struct PushServer {
    stores: HashMap<String, PushStoreInfo>,
}

impl PushServer {
    pub fn new(
        configs: &[WithInstanceName<PushConfig>],
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut stores = HashMap::with_capacity(configs.len());
        for config in configs {
            let store = store_manager.get_store(&config.push_store).ok_or_else(|| {
                make_input_err!("'push_store': '{}' does not exist", config.push_store)
            })?;
            stores.insert(
                config.instance_name.to_string(),
                PushStoreInfo {
                    store,
                    read_only: config.read_only,
                },
            );
        }
        Ok(Self {
            stores: stores.clone(),
        })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    async fn inner_push_blob(
        &self,
        request: PushBlobRequest,
    ) -> Result<Response<PushBlobResponse>, Error> {
        let instance_name = &request.instance_name;
        let store_info = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?;

        if store_info.read_only {
            return Err(make_err!(
                Code::PermissionDenied,
                "The store '{instance_name}' is read only on this endpoint",
            ));
        }

        let blob_digest = request.blob_digest.ok_or_else(|| {
            Error::new(
                Code::InvalidArgument,
                "Missing blob digest in push".to_owned(),
            )
        })?;
        if request.uris.is_empty() {
            return Err(Error::new(
                Code::InvalidArgument,
                "No uris in push request".to_owned(),
            ));
        }
        for uri in request.uris {
            let asset = RemoteAssetArtifact::new(
                uri.clone(),
                request.qualifiers.clone(),
                blob_digest.clone(),
                request.expire_at,
                request.digest_function,
            );
            let asset_digest = asset.digest();
            store_info
                .store
                .update_oneshot(asset_digest, asset.as_bytes())
                .await
                .err_tip(|| "Failed to update in push server")?;
            info!(
                uri = uri,
                digest = format!("{}", asset_digest),
                "Pushed asset metadata with hash"
            );
        }

        Ok(Response::new(PushBlobResponse {}))
    }
}

#[tonic::async_trait]
impl Push for PushServer {
    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err(level = Level::WARN),
        ret(level = Level::INFO),
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn push_blob(
        &self,
        grpc_request: Request<PushBlobRequest>,
    ) -> Result<Response<PushBlobResponse>, Status> {
        let request = grpc_request.into_inner();
        let digest_function = request.digest_function;
        self.inner_push_blob(request)
            .instrument(error_span!("push_push_blob"))
            .with_context(
                make_ctx_for_hash_func(digest_function).err_tip(|| "In PushServer::push_blob")?,
            )
            .await
            .err_tip(|| "Failed on push_blob() command")
            .map_err(Into::into)
    }

    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err(level = Level::WARN),
        ret(level = Level::INFO),
        skip_all,
        fields(request = ?_grpc_request.get_ref())
    )]
    async fn push_directory(
        &self,
        _grpc_request: Request<PushDirectoryRequest>,
    ) -> Result<Response<PushDirectoryResponse>, Status> {
        todo!()
    }
}
