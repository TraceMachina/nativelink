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

use std::convert::Into;

use nativelink_config::cas_server::FetchConfig;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_proto::build::bazel::remote::asset::v1::fetch_server::{
    Fetch, FetchServer as Server,
};
use nativelink_proto::build::bazel::remote::asset::v1::{
    FetchBlobRequest, FetchBlobResponse, FetchDirectoryRequest, FetchDirectoryResponse,
};
use nativelink_store::store_manager::StoreManager;
use nativelink_util::digest_hasher::{default_digest_hasher_func, make_ctx_for_hash_func};
use nativelink_util::origin_event::OriginEventContext;
use tonic::{Request, Response, Status};
use tracing::{Level, error_span, instrument};

#[derive(Debug, Clone, Copy)]
pub struct FetchServer {}

impl FetchServer {
    pub const fn new(_config: &FetchConfig, _store_manager: &StoreManager) -> Result<Self, Error> {
        Ok(FetchServer {})
    }

    pub fn into_service(self) -> Server<FetchServer> {
        Server::new(self)
    }

    async fn inner_fetch_blob(
        &self,
        request: FetchBlobRequest,
    ) -> Result<Response<FetchBlobResponse>, Error> {
        Ok(Response::new(FetchBlobResponse {
            status: Some(make_err!(Code::NotFound, "No item found").into()),
            uri: request.uris.first().cloned().unwrap_or(String::new()),
            qualifiers: vec![],
            expires_at: None,
            blob_digest: None,
            digest_function: default_digest_hasher_func().proto_digest_func().into(),
        }))
    }
}

#[tonic::async_trait]
impl Fetch for FetchServer {
    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn fetch_blob(
        &self,
        grpc_request: Request<FetchBlobRequest>,
    ) -> Result<Response<FetchBlobResponse>, Status> {
        let request = grpc_request.into_inner();
        let ctx = OriginEventContext::new(|| &request).await;
        let resp: Result<Response<FetchBlobResponse>, Status> =
            make_ctx_for_hash_func(request.digest_function)
                .err_tip(|| "In FetchServer::fetch_blob")?
                .wrap_async(
                    error_span!("fetch_server_fetch_blob"),
                    self.inner_fetch_blob(request),
                )
                .await
                .err_tip(|| "Failed on fetch_blob() command")
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
    async fn fetch_directory(
        &self,
        _grpc_request: Request<FetchDirectoryRequest>,
    ) -> Result<Response<FetchDirectoryResponse>, Status> {
        todo!()
    }
}
