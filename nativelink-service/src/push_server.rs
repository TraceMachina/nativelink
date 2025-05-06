// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use core::convert::Into;

use nativelink_config::cas_server::PushConfig;
use nativelink_error::{Error, ResultExt};
use nativelink_proto::build::bazel::remote::asset::v1::push_server::{Push, PushServer as Server};
use nativelink_proto::build::bazel::remote::asset::v1::{
    PushBlobRequest, PushBlobResponse, PushDirectoryRequest, PushDirectoryResponse,
};
use nativelink_store::store_manager::StoreManager;
use nativelink_util::digest_hasher::make_ctx_for_hash_func;
use nativelink_util::origin_event::OriginEventContext;
use tonic::{Request, Response, Status};
use tracing::{Level, error_span, instrument};

#[derive(Debug, Clone, Copy)]
pub struct PushServer {}

impl PushServer {
    pub const fn new(_config: &PushConfig, _store_manager: &StoreManager) -> Result<Self, Error> {
        Ok(Self {})
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    async fn inner_push_blob(
        &self,
        _request: PushBlobRequest,
    ) -> Result<Response<PushBlobResponse>, Error> {
        Ok(Response::new(PushBlobResponse {}))
    }
}

#[tonic::async_trait]
impl Push for PushServer {
    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn push_blob(
        &self,
        grpc_request: Request<PushBlobRequest>,
    ) -> Result<Response<PushBlobResponse>, Status> {
        let request = grpc_request.into_inner();
        let ctx = OriginEventContext::new(|| &request).await;
        let resp: Result<Response<PushBlobResponse>, Status> =
            make_ctx_for_hash_func(request.digest_function)
                .err_tip(|| "In PushServer::push_blob")?
                .wrap_async(error_span!("push_push_blob"), self.inner_push_blob(request))
                .await
                .err_tip(|| "Failed on push_blob() command")
                .map_err(Into::into);
        ctx.emit(|| &resp).await;
        resp
    }

    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
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
