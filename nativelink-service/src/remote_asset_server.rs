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

use nativelink_config::cas_server::RemoteAssetConfig;
use nativelink_error::Error;
use nativelink_proto::build::bazel::remote::asset::v1::fetch_server::{Fetch, FetchServer};
use nativelink_proto::build::bazel::remote::asset::v1::push_server::Push;
use nativelink_proto::build::bazel::remote::asset::v1::{
    FetchBlobRequest, FetchBlobResponse, FetchDirectoryRequest, FetchDirectoryResponse,
};
use nativelink_store::store_manager::StoreManager;
use tonic::{Request, Response, Status};
use tracing::{instrument, Level};

pub struct RemoteAssetServer {}

impl RemoteAssetServer {
    pub fn new(config: &RemoteAssetConfig, store_manager: &StoreManager) -> Result<Self, Error> {
        Ok(RemoteAssetServer {})
    }

    pub fn into_service(self) -> FetchServer<RemoteAssetServer> {
        FetchServer::new(self)
    }
}

#[tonic::async_trait]
impl Fetch for RemoteAssetServer {
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
        todo!()
    }

    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn fetch_directory(
        &self,
        grpc_request: Request<FetchDirectoryRequest>,
    ) -> Result<Response<FetchDirectoryResponse>, Status> {
        todo!()
    }
}
