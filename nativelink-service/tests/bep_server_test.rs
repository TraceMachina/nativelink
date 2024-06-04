// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use std::sync::Arc;

use nativelink_config::cas_server::BepConfig;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_service::bep_server::BepServer;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use prometheus_client::registry::Registry;
use tonic::Request;

const BEP_STORE_NAME: &str = "main_bep";

async fn make_store_manager() -> Result<Arc<StoreManager>, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        BEP_STORE_NAME,
        store_factory(
            &nativelink_config::stores::StoreConfig::memory(
                nativelink_config::stores::MemoryStore::default(),
            ),
            &store_manager,
            Some(&mut <Registry>::default()),
            None,
        )
        .await?,
    );
    Ok(store_manager)
}

fn make_bep_server(store_manager: &StoreManager) -> Result<BepServer, Error> {
    BepServer::new(
        &BepConfig {
            store: BEP_STORE_NAME.to_string(),
        },
        store_manager,
    )
}

#[cfg(test)]
mod event_request {
    use nativelink_proto::google::devtools::build::v1::build_event::console_output::Output;
    use nativelink_proto::google::devtools::build::v1::build_event::{ConsoleOutput, Event};
    use nativelink_proto::google::devtools::build::v1::publish_build_event_server::PublishBuildEvent;
    use nativelink_proto::google::devtools::build::v1::publish_lifecycle_event_request::ServiceLevel;
    use nativelink_proto::google::devtools::build::v1::stream_id::BuildComponent;
    use nativelink_proto::google::devtools::build::v1::{
        BuildEvent, ConsoleOutputStream, OrderedBuildEvent, PublishLifecycleEventRequest, StreamId,
    };
    use nativelink_service::bep_server::get_stream_digest;
    use nativelink_util::buf_channel::make_buf_channel_pair;
    use pretty_assertions::assert_eq;
    use prost::Message;
    use prost_types::Timestamp;

    use super::*;

    /// Asserts that a gRPC request for a [`PublishLifecycleEventRequest`] is correctly dumped into a [`Store`]
    #[nativelink_test]
    async fn bep_server_emits_events() -> Result<(), Box<dyn std::error::Error>> {
        let store_manager = make_store_manager().await?;
        let bep_server = make_bep_server(&store_manager)?;
        let bep_store = store_manager.get_store(BEP_STORE_NAME).unwrap();

        let stream_id = StreamId {
            build_id: "some-build-id".to_string(),
            invocation_id: "some-invocation-id".to_string(),
            component: BuildComponent::Controller as i32,
        };
        let digest = get_stream_digest(&stream_id).unwrap();

        let request = PublishLifecycleEventRequest {
            service_level: ServiceLevel::Interactive as i32,
            build_event: Some(OrderedBuildEvent {
                stream_id: Some(stream_id),
                sequence_number: 1,
                event: Some(BuildEvent {
                    event_time: Some(Timestamp::date(1999, 1, 6).unwrap()),
                    event: Some(Event::ConsoleOutput(ConsoleOutput {
                        r#type: ConsoleOutputStream::Stdout as i32,
                        output: Some(Output::TextOutput(
                            "Here's some text that's been printed to stdout".to_string(),
                        )),
                    })),
                }),
            }),
            stream_timeout: None,
            notification_keywords: vec!["testing".to_string(), "console".to_string()],
            project_id: "some-project-id".to_string(),
            check_preceding_lifecycle_events_present: false,
        };

        let raw_response = bep_server
            .publish_lifecycle_event(Request::new(request.clone()))
            .await;

        assert!(raw_response.is_ok());

        let (writer, mut reader) = make_buf_channel_pair();

        let part_result = bep_store.get_part_arc(digest, writer, 0, None).await;
        assert!(part_result.is_ok());

        let read_result = reader.recv().await;
        assert!(read_result.is_ok());

        let bytes = read_result.unwrap();
        let decoded_request_result = PublishLifecycleEventRequest::decode(bytes);
        assert!(decoded_request_result.is_ok());

        let decoded_request = decoded_request_result.unwrap();
        assert_eq!(request, decoded_request);

        Ok(())
    }
}

#[cfg(test)]
mod event_request_stream {
    use std::io;
    use std::pin::Pin;

    use futures::{Future, Sink, SinkExt, Stream, StreamExt};
    use hyper::Body;
    use nativelink_proto::google::devtools::build::v1::build_event::console_output::Output;
    use nativelink_proto::google::devtools::build::v1::build_event::{
        BuildEnqueued, BuildFinished, ConsoleOutput, Event, InvocationAttemptFinished,
        InvocationAttemptStarted,
    };
    use nativelink_proto::google::devtools::build::v1::publish_build_event_server::PublishBuildEvent;
    use nativelink_proto::google::devtools::build::v1::stream_id::BuildComponent;
    use nativelink_proto::google::devtools::build::v1::{
        build_status, BuildEvent, BuildStatus, ConsoleOutputStream, OrderedBuildEvent,
        PublishBuildToolEventStreamRequest, PublishBuildToolEventStreamResponse, StreamId,
    };
    use nativelink_service::bep_server::get_stream_digest;
    use nativelink_util::buf_channel::make_buf_channel_pair;
    use nativelink_util::common::encode_stream_proto;
    use nativelink_util::spawn;
    use pretty_assertions::assert_eq;
    use prost::Message;
    use prost_types::Timestamp;
    use tonic::codec::{Codec, CompressionEncoding, ProstCodec};
    use tonic::{Status, Streaming};

    use super::*;

    #[nativelink_test]
    async fn bep_server_emits_events_and_responds() -> Result<(), Box<dyn std::error::Error>> {
        let store_manager = make_store_manager().await?;
        let bep_server = make_bep_server(&store_manager)?;
        let bep_store = store_manager.get_store(BEP_STORE_NAME).unwrap();

        // Utility function to abstract over the underlying HTTP / gRPC stack to make tests more readable.
        let make_request = {
            let (mut tx, body) = Body::channel();
            let mut codec = ProstCodec::<
                PublishBuildToolEventStreamRequest,
                PublishBuildToolEventStreamRequest,
            >::default();

            // Note: This is an undocumented function.
            let stream = Streaming::<PublishBuildToolEventStreamRequest>::new_request(
                codec.decoder(),
                body,
                Some(CompressionEncoding::Gzip),
                None,
            );

            let join_handle = spawn!(
                "bep_server_emits_events_and_responds_server_task",
                async move {
                    bep_server
                        .publish_build_tool_event_stream(Request::new(stream))
                        .await
                },
            );

            let mut response_stream = join_handle.await??.into_inner();

            Ok::<_, Box<dyn std::error::Error>>(
                move |request: &PublishBuildToolEventStreamRequest| async move {
                    tx.send_data(encode_stream_proto(request)?).await?;
                    response_stream.next().await.ok_or_else(|| {
                        Box::new(io::Error::new(
                            io::ErrorKind::ConnectionAborted,
                            "Couldnt retrieve anything from response stream.",
                        )) as Box<dyn std::error::Error>
                    })
                },
            )
        }?;

        // Construct requests to send to the server
        let stream_id = StreamId {
            build_id: "some-build-id".to_string(),
            invocation_id: "some-invocation-id".to_string(),
            component: BuildComponent::Controller as i32,
        };
        let digest = get_stream_digest(&stream_id).unwrap();
        let project_id = "some-project-id".to_string();

        let requests = [
            PublishBuildToolEventStreamRequest {
                ordered_build_event: Some(OrderedBuildEvent {
                    stream_id: Some(stream_id.clone()),
                    sequence_number: 1,
                    event: Some(BuildEvent {
                        event_time: Some(Timestamp::date(1999, 1, 4).unwrap()),
                        event: Some(Event::BuildEnqueued(BuildEnqueued { details: None })),
                    }),
                }),
                notification_keywords: vec!["testing".to_string(), "build-enqueued".to_string()],
                project_id: project_id.clone(),
                check_preceding_lifecycle_events_present: false,
            },
            PublishBuildToolEventStreamRequest {
                ordered_build_event: Some(OrderedBuildEvent {
                    stream_id: Some(stream_id.clone()),
                    sequence_number: 2,
                    event: Some(BuildEvent {
                        event_time: Some(Timestamp::date(1999, 1, 5).unwrap()),
                        event: Some(Event::InvocationAttemptStarted(InvocationAttemptStarted {
                            attempt_number: 1,
                            details: None,
                        })),
                    }),
                }),
                notification_keywords: vec!["testing".to_string()],
                project_id: project_id.clone(),
                check_preceding_lifecycle_events_present: false,
            },
            PublishBuildToolEventStreamRequest {
                ordered_build_event: Some(OrderedBuildEvent {
                    stream_id: Some(stream_id.clone()),
                    sequence_number: 3,
                    event: Some(BuildEvent {
                        event_time: Some(Timestamp::date(1999, 1, 6).unwrap()),
                        event: Some(Event::ConsoleOutput(ConsoleOutput {
                            r#type: ConsoleOutputStream::Stdout as i32,
                            output: Some(Output::TextOutput(
                                "This is taking a while...".to_string(),
                            )),
                        })),
                    }),
                }),
                notification_keywords: vec!["testing".to_string()],
                project_id: project_id.clone(),
                check_preceding_lifecycle_events_present: false,
            },
            PublishBuildToolEventStreamRequest {
                ordered_build_event: Some(OrderedBuildEvent {
                    stream_id: Some(stream_id.clone()),
                    sequence_number: 4,
                    event: Some(BuildEvent {
                        event_time: Some(Timestamp::date(1999, 1, 7).unwrap()),
                        event: Some(Event::InvocationAttemptFinished(
                            InvocationAttemptFinished {
                                invocation_status: Some(BuildStatus {
                                    result: build_status::Result::InvocationDeadlineExceeded as i32,
                                    final_invocation_id: String::default(),
                                    build_tool_exit_code: Some(1),
                                    error_message: "You missed my birthday!".to_string(),
                                    details: None,
                                }),
                                details: None,
                            },
                        )),
                    }),
                }),
                notification_keywords: vec!["testing".to_string()],
                project_id: "some-project-id".to_string(),
                check_preceding_lifecycle_events_present: false,
            },
            PublishBuildToolEventStreamRequest {
                ordered_build_event: Some(OrderedBuildEvent {
                    stream_id: Some(stream_id.clone()),
                    sequence_number: 5,
                    event: Some(BuildEvent {
                        event_time: Some(Timestamp::date(1999, 1, 8).unwrap()),
                        event: Some(Event::BuildFinished(BuildFinished {
                            status: Some(BuildStatus {
                                result: build_status::Result::InvocationDeadlineExceeded as i32,
                                final_invocation_id: String::default(),
                                build_tool_exit_code: Some(1),
                                error_message: "Missed her birthday...".to_string(),
                                details: None,
                            }),
                            details: None,
                        })),
                    }),
                }),
                notification_keywords: vec!["testing".to_string()],
                project_id: project_id.clone(),
                check_preceding_lifecycle_events_present: false,
            },
        ];

        // now we check the responses!
        // there's two parts to this API:
        // 1. The server responds to the client with confirmation of the sequence numbers
        // 2. The server dumps the events into a [`Store`]
        // we want to check both parts were executed correctly

        for (i, req) in requests.iter().enumerate() {
            let response = make_request(req).await??;

            // First, check if the response matches what we expect
            assert_eq!(response.sequence_number, i as i64);
            assert_eq!(response.stream_id, Some(stream_id.clone()));

            // Second, check if the message was forwarded correctly
            let (writer, mut reader) = make_buf_channel_pair();
            let offset = requests
                .iter()
                .take(i)
                .map(|request| request.encoded_len())
                .sum();
            let length = req.encoded_len();

            bep_store
                .clone()
                .get_part_arc(digest, writer, offset, Some(length))
                .await?;
            let encoded_request = reader.recv().await?;

            let decoded_request = PublishBuildToolEventStreamRequest::decode(encoded_request)?;

            assert_eq!(*req, decoded_request);
        }

        Ok(())
    }
}
