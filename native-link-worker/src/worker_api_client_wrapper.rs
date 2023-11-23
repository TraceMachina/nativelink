// Copyright 2023 The Native Link Authors. All rights reserved.
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

use async_trait::async_trait;
use proto::com::github::trace_machina::native_link::remote_execution::worker_api_client::WorkerApiClient;
use proto::com::github::trace_machina::native_link::remote_execution::{
    ExecuteResult, GoingAwayRequest, KeepAliveRequest, SupportedProperties, UpdateForWorker,
};
use tonic::codec::Streaming;
use tonic::transport::Channel;
use tonic::{Response, Status};

/// This is used in order to allow unit tests to intercept these calls. This should always match
/// the API of WorkerApiClient defined in the worker_api.proto file.
#[async_trait]
pub trait WorkerApiClientTrait: Clone + Sync + Send + Sized + Unpin {
    async fn connect_worker(
        &mut self,
        request: SupportedProperties,
    ) -> Result<Response<Streaming<UpdateForWorker>>, Status>;

    async fn keep_alive(&mut self, request: KeepAliveRequest) -> Result<Response<()>, Status>;

    async fn going_away(&mut self, request: GoingAwayRequest) -> Result<Response<()>, Status>;

    async fn execution_response(&mut self, request: ExecuteResult) -> Result<Response<()>, Status>;
}

#[derive(Clone)]
pub struct WorkerApiClientWrapper {
    inner: WorkerApiClient<Channel>,
}

impl From<WorkerApiClient<Channel>> for WorkerApiClientWrapper {
    fn from(other: WorkerApiClient<Channel>) -> Self {
        Self { inner: other }
    }
}

#[async_trait]
impl WorkerApiClientTrait for WorkerApiClientWrapper {
    async fn connect_worker(
        &mut self,
        request: SupportedProperties,
    ) -> Result<Response<Streaming<UpdateForWorker>>, Status> {
        self.inner.connect_worker(request).await
    }

    async fn keep_alive(&mut self, request: KeepAliveRequest) -> Result<Response<()>, Status> {
        self.inner.keep_alive(request).await
    }

    async fn going_away(&mut self, request: GoingAwayRequest) -> Result<Response<()>, Status> {
        self.inner.going_away(request).await
    }

    async fn execution_response(&mut self, request: ExecuteResult) -> Result<Response<()>, Status> {
        self.inner.execution_response(request).await
    }
}
