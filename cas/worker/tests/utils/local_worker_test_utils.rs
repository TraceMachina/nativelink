// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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
use std::sync::Arc;

use hyper::body::Sender as HyperSender;
use tonic::{
    codec::Codec, // Needed for .decoder().
    codec::CompressionEncoding,
    codec::ProstCodec,
    transport::Body,
    Response,
    Streaming,
};

use common::JoinHandleDropGuard;
use config::cas_server::{EndpointConfig, LocalWorkerConfig, WorkerProperty};
use error::Error;
use local_worker::LocalWorker;
use mock_running_actions_manager::MockRunningActionsManager;
use mock_worker_api_client::MockWorkerApiClient;
use proto::com::github::trace_machina::turbo_cache::remote_execution::UpdateForWorker;

pub fn setup_grpc_stream() -> (HyperSender, Response<Streaming<UpdateForWorker>>) {
    let (tx, body) = Body::channel();
    let mut codec = ProstCodec::<UpdateForWorker, UpdateForWorker>::default();
    // Note: This is an undocumented function.
    let stream = Streaming::new_request(codec.decoder(), body, Some(CompressionEncoding::Gzip), None);
    (tx, Response::new(stream))
}

pub async fn setup_local_worker_with_config(local_worker_config: LocalWorkerConfig) -> TestContext {
    let mock_worker_api_client = MockWorkerApiClient::new();
    let mock_worker_api_client_clone = mock_worker_api_client.clone();
    let actions_manager = Arc::new(MockRunningActionsManager::new());
    let worker = LocalWorker::new_with_connection_factory_and_actions_manager(
        Arc::new(local_worker_config),
        actions_manager.clone(),
        Box::new(move || {
            let mock_worker_api_client = mock_worker_api_client_clone.clone();
            Box::pin(async move { Ok(mock_worker_api_client) })
        }),
        Box::new(move |_| Box::pin(async move { /* No sleep */ })),
    );
    let drop_guard = JoinHandleDropGuard::new(tokio::spawn(async move { worker.run().await }));

    let (tx_stream, streaming_response) = setup_grpc_stream();
    TestContext {
        client: mock_worker_api_client,
        actions_manager,

        maybe_streaming_response: Some(streaming_response),
        maybe_tx_stream: Some(tx_stream),

        _drop_guard: drop_guard,
    }
}

pub async fn setup_local_worker(platform_properties: HashMap<String, WorkerProperty>) -> TestContext {
    const ARBITRARY_LARGE_TIMEOUT: f32 = 10000.;
    let local_worker_config = LocalWorkerConfig {
        platform_properties,
        worker_api_endpoint: EndpointConfig {
            timeout: Some(ARBITRARY_LARGE_TIMEOUT),
            ..Default::default()
        },
        ..Default::default()
    };
    setup_local_worker_with_config(local_worker_config).await
}

pub struct TestContext {
    pub client: MockWorkerApiClient,
    pub actions_manager: Arc<MockRunningActionsManager>,

    pub maybe_streaming_response: Option<Response<Streaming<UpdateForWorker>>>,
    pub maybe_tx_stream: Option<HyperSender>,

    _drop_guard: JoinHandleDropGuard<Result<(), Error>>,
}
