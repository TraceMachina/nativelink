// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use bytes::BytesMut;
use futures::stream::unfold;
use futures::Stream;
use nativelink_error::{Error, ResultExt};
use nativelink_proto::google::devtools::build::v1::publish_build_event_server::{
    PublishBuildEvent, PublishBuildEventServer,
};
use nativelink_proto::google::devtools::build::v1::{
    PublishBuildToolEventStreamRequest, PublishBuildToolEventStreamResponse,
    PublishLifecycleEventRequest, StreamId,
};
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::store_trait::Store;
use prost::Message;
use tonic::{Request, Response, Result, Status, Streaming};
use tracing::{instrument, Level};

pub struct BepServer {
    store: Arc<dyn Store>,
}

fn get_stream_digest(stream_id: &StreamId) -> Result<DigestInfo, Error> {
    let mut hasher = DigestHasherFunc::Blake3.hasher();
    hasher.update(stream_id.build_id.as_bytes());
    hasher.update(stream_id.invocation_id.as_bytes());
    hasher.update(format!("{}", stream_id.component).as_bytes());
    Ok(hasher.finalize_digest())
}

impl BepServer {
    pub fn new(
        config: &nativelink_config::cas_server::BepConfig,
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let store = store_manager
            .get_store(&config.store)
            .err_tip(|| format!("Expected store {} to exist in store manager", &config.store))?;

        Ok(Self { store })
    }

    pub fn into_service(self) -> PublishBuildEventServer<BepServer> {
        PublishBuildEventServer::new(self)
    }

    async fn inner_publish_lifecycle_event(
        &self,
        request: PublishLifecycleEventRequest,
    ) -> Result<Response<()>, Error> {
        let build_event = request
            .build_event
            .as_ref()
            .err_tip(|| "Expected build_event to be set")?;
        let stream_id = build_event
            .stream_id
            .as_ref()
            .err_tip(|| "Expected stream_id to be set")?;
        let digest_info =
            get_stream_digest(stream_id).err_tip(|| "Failed to prepare request for upload")?;

        let mut buf = BytesMut::new();
        request
            .encode(&mut buf)
            .err_tip(|| "Could not encode PublishLifecycleEventRequest proto")?;

        Pin::new(self.store.as_ref())
            .update_oneshot(digest_info, buf.freeze())
            .await
            .err_tip(|| "Failed to store PublishLifecycleEventRequest")?;

        Ok(Response::new(()))
    }

    async fn inner_publish_build_tool_event_stream(
        &self,
        stream: Streaming<PublishBuildToolEventStreamRequest>,
    ) -> Result<Response<PublishBuildToolEventStreamStream>, Error> {
        async fn process_request(
            store: Pin<&dyn Store>,
            request: PublishBuildToolEventStreamRequest,
        ) -> Result<PublishBuildToolEventStreamResponse, Status> {
            let ordered_build_event = request
                .ordered_build_event
                .as_ref()
                .err_tip(|| "Expected ordered_build_event to be set")?;
            let stream_id = ordered_build_event
                .stream_id
                .as_ref()
                .err_tip(|| "Expected stream_id to be set")?;

            let sequence_number = ordered_build_event.sequence_number;

            let digest_info =
                get_stream_digest(stream_id).err_tip(|| "Failed to prepare request for upload")?;

            let mut buf = BytesMut::new();
            request
                .encode(&mut buf)
                .err_tip(|| "Could not encode PublishBuildToolEventStreamRequest proto")?;

            store
                .update_oneshot(digest_info, buf.freeze())
                .await
                .err_tip(|| "Failed to store PublishBuildToolEventStreamRequest")?;

            Ok(PublishBuildToolEventStreamResponse {
                stream_id: Some(stream_id.clone()),
                sequence_number,
            })
        }
        struct State {
            store: Arc<dyn Store>,
            stream: Streaming<PublishBuildToolEventStreamRequest>,
        }
        Ok(Response::new(Box::pin(unfold(
            Some(State {
                store: self.store.clone(),
                stream,
            }),
            move |maybe_state| async move {
                let mut state = maybe_state?;
                let request = match state
                    .stream
                    .message()
                    .await
                    .err_tip(|| "While receiving message in publish_build_tool_event_stream")
                {
                    Ok(Some(request)) => request,
                    Ok(None) => return None,
                    Err(e) => return Some((Err(e.into()), None)),
                };
                process_request(Pin::new(state.store.as_ref()), request)
                    .await
                    .map_or_else(|e| Some((Err(e), None)), |result| Some((Ok(result), None)))
            },
        ))))
    }
}

type PublishBuildToolEventStreamStream = Pin<
    Box<dyn Stream<Item = Result<PublishBuildToolEventStreamResponse, Status>> + Send + 'static>,
>;

#[tonic::async_trait]
impl PublishBuildEvent for BepServer {
    type PublishBuildToolEventStreamStream = PublishBuildToolEventStreamStream;

    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn publish_lifecycle_event(
        &self,
        grpc_request: Request<PublishLifecycleEventRequest>,
    ) -> Result<Response<()>, Status> {
        self.inner_publish_lifecycle_event(grpc_request.into_inner())
            .await
            .map_err(|e| e.into())
    }

    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
      err,
      level = Level::ERROR,
      skip_all,
      fields(request = ?grpc_request.get_ref())
    )]
    async fn publish_build_tool_event_stream(
        &self,
        grpc_request: Request<Streaming<PublishBuildToolEventStreamRequest>>,
    ) -> Result<Response<Self::PublishBuildToolEventStreamStream>, Status> {
        self.inner_publish_build_tool_event_stream(grpc_request.into_inner())
            .await
            .map_err(|e| e.into())
    }
}
