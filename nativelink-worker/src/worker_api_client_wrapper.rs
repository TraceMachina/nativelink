// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::future::Future;

use futures::stream::unfold;
use nativelink_error::{make_err, Error, ResultExt};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_scheduler::Update;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::worker_api_client::WorkerApiClient;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    ConnectWorkerRequest, ExecuteComplete, ExecuteResult, GoingAwayRequest, KeepAliveRequest, UpdateForScheduler, UpdateForWorker
};
use tokio::sync::mpsc::Sender;
use tonic::codec::Streaming;
use tonic::transport::Channel;
use tonic::{Code, Response, Status};

/// This is used in order to allow unit tests to intercept these calls. This should always match
/// the API of `WorkerApiClient` defined in the `worker_api.proto` file.
pub trait WorkerApiClientTrait: Clone + Sync + Send + Sized + Unpin {
    fn connect_worker(
        &mut self,
        request: ConnectWorkerRequest,
    ) -> impl Future<Output = Result<Response<Streaming<UpdateForWorker>>, Status>> + Send;

    fn keep_alive(
        &mut self,
        request: KeepAliveRequest,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    fn going_away(
        &mut self,
        request: GoingAwayRequest,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    fn execution_response(
        &mut self,
        request: ExecuteResult,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    fn execution_complete(
        &mut self,
        request: ExecuteComplete,
    ) -> impl Future<Output = Result<(), Error>> + Send;
}

#[derive(Debug, Clone)]
pub struct WorkerApiClientWrapper {
    inner: WorkerApiClient<Channel>,
    channel: Option<Sender<Update>>,
}

impl WorkerApiClientWrapper {
    async fn send_update(&mut self, update: Update) -> Result<(), Error> {
        let tx = self
            .channel
            .as_ref()
            .err_tip(|| "worker update without connect_worker")?;
        match tx.send(update).await {
            Ok(()) => Ok(()),
            Err(_err) => {
                // Remove the sender if it's not going anywhere.
                self.channel.take();
                Err(make_err!(
                    Code::Unavailable,
                    "worker update with disconnected channel"
                ))
            }
        }
    }
}

impl From<WorkerApiClient<Channel>> for WorkerApiClientWrapper {
    fn from(other: WorkerApiClient<Channel>) -> Self {
        Self {
            inner: other,
            channel: None,
        }
    }
}

impl WorkerApiClientTrait for WorkerApiClientWrapper {
    async fn connect_worker(
        &mut self,
        request: ConnectWorkerRequest,
    ) -> Result<Response<Streaming<UpdateForWorker>>, Status> {
        drop(self.channel.take());
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        if tx
            .send(Update::ConnectWorkerRequest(request))
            .await
            .is_err()
        {
            return Err(Status::data_loss("Unable to push to newly created channel"));
        }
        self.channel = Some(tx);
        self.inner
            .connect_worker(unfold(rx, |mut rx| async move {
                let update = rx.recv().await?;
                Some((
                    UpdateForScheduler {
                        update: Some(update),
                    },
                    rx,
                ))
            }))
            .await
    }

    async fn keep_alive(&mut self, request: KeepAliveRequest) -> Result<(), Error> {
        self.send_update(Update::KeepAliveRequest(request)).await
    }

    async fn going_away(&mut self, request: GoingAwayRequest) -> Result<(), Error> {
        self.send_update(Update::GoingAwayRequest(request)).await
    }

    async fn execution_response(&mut self, request: ExecuteResult) -> Result<(), Error> {
        self.send_update(Update::ExecuteResult(request)).await
    }

    async fn execution_complete(&mut self, request: ExecuteComplete) -> Result<(), Error> {
        self.send_update(Update::ExecuteComplete(request)).await
    }
}
