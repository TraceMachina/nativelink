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

use std::borrow::Cow;
use std::pin::Pin;

use bytes::BytesMut;
use futures::stream::unfold;
use futures::Stream;
use nativelink_error::{Error, ResultExt};
use nativelink_proto::com::github::trace_machina::nativelink::events::{bep_event, BepEvent};
use nativelink_proto::google::devtools::build::v1::publish_build_event_server::{
    PublishBuildEvent, PublishBuildEventServer,
};
use nativelink_proto::google::devtools::build::v1::{
    PublishBuildToolEventStreamRequest, PublishBuildToolEventStreamResponse,
    PublishLifecycleEventRequest,
};
use nativelink_store::store_manager::StoreManager;
use nativelink_util::origin_context::{ActiveOriginContext, ORIGIN_IDENTITY};
use nativelink_util::store_trait::{Store, StoreDriver, StoreKey, StoreLike};
use prost::Message;
use tonic::{Request, Response, Result, Status, Streaming};
use tracing::{instrument, Level};

/// Current version of the BEP event. This might be used in the future if
/// there is a breaking change in the BEP event format.
const BEP_EVENT_VERSION: u32 = 0;

fn get_identity() -> Result<Option<String>, Status> {
    ActiveOriginContext::get()
        .map_or(Ok(None), |ctx| ctx.get_value(&ORIGIN_IDENTITY))
        .err_tip(|| "In BepServer")
        .map_or_else(|e| Err(e.into()), |v| Ok(v.map(|v| v.as_ref().clone())))
}

pub struct BepServer {
    store: Store,
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
        identity: Option<String>,
    ) -> Result<Response<()>, Error> {
        let build_event = request
            .build_event
            .as_ref()
            .err_tip(|| "Expected build_event to be set")?;
        let stream_id = build_event
            .stream_id
            .as_ref()
            .err_tip(|| "Expected stream_id to be set")?;

        let sequence_number = build_event.sequence_number;

        let store_key = StoreKey::Str(Cow::Owned(format!(
            "BepEvent:le:{}:{}:{}",
            &stream_id.build_id, &stream_id.invocation_id, sequence_number,
        )));

        let bep_event = BepEvent {
            version: BEP_EVENT_VERSION,
            identity: identity.unwrap_or_default(),
            event: Some(bep_event::Event::LifecycleEvent(request)),
        };
        let mut buf = BytesMut::new();
        bep_event
            .encode(&mut buf)
            .err_tip(|| "Could not encode PublishLifecycleEventRequest proto")?;

        self.store
            .update_oneshot(store_key, buf.freeze())
            .await
            .err_tip(|| "Failed to store PublishLifecycleEventRequest")?;

        Ok(Response::new(()))
    }

    async fn inner_publish_build_tool_event_stream(
        &self,
        stream: Streaming<PublishBuildToolEventStreamRequest>,
        identity: Option<String>,
    ) -> Result<Response<PublishBuildToolEventStreamStream>, Error> {
        async fn process_request(
            store: Pin<&dyn StoreDriver>,
            request: PublishBuildToolEventStreamRequest,
            identity: String,
        ) -> Result<PublishBuildToolEventStreamResponse, Status> {
            let ordered_build_event = request
                .ordered_build_event
                .as_ref()
                .err_tip(|| "Expected ordered_build_event to be set")?;
            let stream_id = ordered_build_event
                .stream_id
                .as_ref()
                .err_tip(|| "Expected stream_id to be set")?
                .clone();

            let sequence_number = ordered_build_event.sequence_number;

            let bep_event = BepEvent {
                version: BEP_EVENT_VERSION,
                identity,
                event: Some(bep_event::Event::BuildToolEvent(request)),
            };
            let mut buf = BytesMut::new();

            bep_event
                .encode(&mut buf)
                .err_tip(|| "Could not encode PublishBuildToolEventStreamRequest proto")?;

            store
                .update_oneshot(
                    StoreKey::Str(Cow::Owned(format!(
                        "BepEvent:be:{}:{}:{}",
                        &stream_id.build_id, &stream_id.invocation_id, sequence_number,
                    ))),
                    buf.freeze(),
                )
                .await
                .err_tip(|| "Failed to store PublishBuildToolEventStreamRequest")?;

            Ok(PublishBuildToolEventStreamResponse {
                stream_id: Some(stream_id.clone()),
                sequence_number,
            })
        }

        struct State {
            store: Store,
            stream: Streaming<PublishBuildToolEventStreamRequest>,
            identity: String,
        }

        let response_stream =
            unfold(
                Some(State {
                    store: self.store.clone(),
                    stream,
                    identity: identity.unwrap_or_default(),
                }),
                move |maybe_state| async move {
                    let mut state = maybe_state?;
                    let request =
                        match state.stream.message().await.err_tip(|| {
                            "While receiving message in publish_build_tool_event_stream"
                        }) {
                            Ok(Some(request)) => request,
                            Ok(None) => return None,
                            Err(e) => return Some((Err(e.into()), None)),
                        };
                    process_request(
                        state.store.as_store_driver_pin(),
                        request,
                        state.identity.clone(),
                    )
                    .await
                    .map_or_else(
                        |e| Some((Err(e), None)),
                        |response| Some((Ok(response), Some(state))),
                    )
                },
            );

        Ok(Response::new(Box::pin(response_stream)))
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
        self.inner_publish_lifecycle_event(grpc_request.into_inner(), get_identity()?)
            .await
            .map_err(Error::into)
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
        self.inner_publish_build_tool_event_stream(grpc_request.into_inner(), get_identity()?)
            .await
            .map_err(Error::into)
    }
}
