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

use std::sync::OnceLock;

use base64::Engine;
use base64::prelude::BASE64_STANDARD_NO_PAD;
use nativelink_proto::build::bazel::remote::execution::v2::RequestMetadata;
use nativelink_proto::com::github::trace_machina::nativelink::events::{
    Event, event, request_event, response_event, stream_event,
};
use prost::Message;
use rand::RngCore;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

static NODE_ID: OnceLock<[u8; 6]> = OnceLock::new();

/// Returns a unique ID for the given event.
/// This ID is used to identify the event type.
/// The max value that could be output is 0x0FFF,
/// meaning you may use the first nibble for other
/// purposes.
#[inline]
pub const fn get_id_for_event(event: &Event) -> [u8; 2] {
    match &event.event {
        None => [0x00, 0x00],
        Some(event::Event::Request(req)) => match req.event {
            None => [0x01, 0x00],
            Some(request_event::Event::GetCapabilitiesRequest(_)) => [0x01, 0x01],
            Some(request_event::Event::GetActionResultRequest(_)) => [0x01, 0x02],
            Some(request_event::Event::UpdateActionResultRequest(_)) => [0x01, 0x03],
            Some(request_event::Event::FindMissingBlobsRequest(_)) => [0x01, 0x04],
            Some(request_event::Event::BatchReadBlobsRequest(_)) => [0x01, 0x05],
            Some(request_event::Event::BatchUpdateBlobsRequest(_)) => [0x01, 0x06],
            Some(request_event::Event::GetTreeRequest(_)) => [0x01, 0x07],
            Some(request_event::Event::ReadRequest(_)) => [0x01, 0x08],
            Some(request_event::Event::WriteRequest(())) => [0x01, 0x09],
            Some(request_event::Event::QueryWriteStatusRequest(_)) => [0x01, 0x0A],
            Some(request_event::Event::ExecuteRequest(_)) => [0x01, 0x0B],
            Some(request_event::Event::WaitExecutionRequest(_)) => [0x01, 0x0C],
            Some(request_event::Event::SchedulerStartExecute(_)) => [0x01, 0x0D],
        },
        Some(event::Event::Response(res)) => match res.event {
            None => [0x02, 0x00],
            Some(response_event::Event::Error(_)) => [0x02, 0x01],
            Some(response_event::Event::ServerCapabilities(_)) => [0x02, 0x02],
            Some(response_event::Event::ActionResult(_)) => [0x02, 0x03],
            Some(response_event::Event::FindMissingBlobsResponse(_)) => [0x02, 0x04],
            Some(response_event::Event::BatchReadBlobsResponse(_)) => [0x02, 0x05],
            Some(response_event::Event::BatchUpdateBlobsResponse(_)) => [0x02, 0x06],
            Some(response_event::Event::WriteResponse(_)) => [0x02, 0x07],
            Some(response_event::Event::QueryWriteStatusResponse(_)) => [0x02, 0x08],
            Some(response_event::Event::Empty(())) => [0x02, 0x09],
        },
        Some(event::Event::Stream(stream)) => match stream.event {
            None => [0x03, 0x00],
            Some(stream_event::Event::Error(_)) => [0x03, 0x01],
            Some(stream_event::Event::GetTreeResponse(_)) => [0x03, 0x02],
            Some(stream_event::Event::DataLength(_)) => [0x03, 0x03],
            Some(stream_event::Event::WriteRequest(_)) => [0x03, 0x04],
            Some(stream_event::Event::Operation(_)) => [0x03, 0x05],
            Some(stream_event::Event::Closed(())) => [0x03, 0x06], // Special case when stream has terminated.
        },
    }
}

/// Returns a unique node ID for this process.
pub fn get_node_id(event: Option<&Event>) -> [u8; 6] {
    let mut node_id = *NODE_ID.get_or_init(|| {
        let mut out = [0; 6];
        rand::rng().fill_bytes(&mut out);
        out
    });
    let Some(event) = event else {
        return node_id;
    };
    let event_id = get_id_for_event(event);
    node_id[0] = (node_id[0] & 0xF0) | event_id[0];
    node_id[1] = event_id[1];
    node_id
}

fn serialize_request_metadata<S>(
    value: &Option<RequestMetadata>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(msg) => serializer.serialize_some(&BASE64_STANDARD_NO_PAD.encode(msg.encode_to_vec())),
        None => serializer.serialize_none(),
    }
}

fn deserialize_request_metadata<'de, D>(
    deserializer: D,
) -> Result<Option<RequestMetadata>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt {
        Some(s) => {
            let decoded = BASE64_STANDARD_NO_PAD
                .decode(s.as_bytes())
                .map_err(serde::de::Error::custom)?;
            RequestMetadata::decode(&*decoded)
                .map_err(serde::de::Error::custom)
                .map(Some)
        }
        None => Ok(None),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OriginMetadata {
    pub identity: String,
    #[serde(
        serialize_with = "serialize_request_metadata",
        deserialize_with = "deserialize_request_metadata"
    )]
    pub bazel_metadata: Option<RequestMetadata>,
}
