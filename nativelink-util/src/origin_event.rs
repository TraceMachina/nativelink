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

use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use futures::future::ready;
use futures::{Future, FutureExt, Stream, StreamExt};
use nativelink_proto::build::bazel::remote::execution::v2::{
    ActionResult, BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, ExecuteRequest, FindMissingBlobsRequest, FindMissingBlobsResponse,
    GetActionResultRequest, GetCapabilitiesRequest, GetTreeRequest, GetTreeResponse,
    RequestMetadata, ServerCapabilities, UpdateActionResultRequest, WaitExecutionRequest,
};
use nativelink_proto::com::github::trace_machina::nativelink::events::origin_event::Event;
use nativelink_proto::com::github::trace_machina::nativelink::events::{
    batch_read_blobs_response_override, batch_update_blobs_request_override,
    execute_stream_response, get_tree_stream_response, read_stream_response, write_stream_request,
    BatchReadBlobsResponseOverride, BatchUpdateBlobsRequestOverride, ExecuteStreamResponse,
    GetTreeStreamResponse, OriginEvent, ReadStreamResponse, WriteRequestOverride,
    WriteStreamRequest,
};
use nativelink_proto::google::bytestream::{
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use nativelink_proto::google::longrunning::Operation;
use nativelink_proto::google::rpc::Status;
use rand::RngCore;
use tokio::sync::mpsc;
use tonic::{Response, Status as TonicStatus, Streaming};
use uuid::Uuid;

use crate::make_symbol;
use crate::origin_context::ActiveOriginContext;

const ORIGIN_EVENT_VERSION: u32 = 0;

static NODE_ID: OnceLock<[u8; 6]> = OnceLock::new();

/// Returns a unique node ID for this process.
pub fn get_node_id() -> &'static [u8; 6] {
    NODE_ID.get_or_init(|| {
        let mut rng = rand::thread_rng();
        let mut out = [0; 6];
        rng.fill_bytes(&mut out);
        out
    })
}

pub struct OriginEventCollector {
    sender: mpsc::Sender<OriginEvent>,
    identity: String,
    bazel_metadata: Option<RequestMetadata>,
}

impl OriginEventCollector {
    pub fn new(
        sender: mpsc::Sender<OriginEvent>,
        identity: String,
        bazel_metadata: Option<RequestMetadata>,
    ) -> Self {
        Self {
            sender,
            identity,
            bazel_metadata,
        }
    }

    async fn publish_origin_event(&self, event: Event, parent_event_id: Option<Uuid>) -> Uuid {
        let event_id = Uuid::now_v6(get_node_id());
        let parent_event_id =
            parent_event_id.map_or_else(String::new, |id| id.as_hyphenated().to_string());
        // Failing to send this event means that the receiver has been dropped.
        let _ = self
            .sender
            .send(OriginEvent {
                version: ORIGIN_EVENT_VERSION,
                event_id: event_id.as_hyphenated().to_string(),
                parent_event_id,
                bazel_request_metadata: self.bazel_metadata.clone(),
                identity: self.identity.clone(),
                event: Some(event),
            })
            .await;
        event_id
    }
}

make_symbol!(ORIGIN_EVENT_COLLECTOR, OriginEventCollector);

pub struct OriginEventContext<T> {
    inner: Option<OriginEventContextImpl>,
    _phantom: PhantomData<T>,
}

impl<T> Clone for OriginEventContext<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _phantom: PhantomData,
        }
    }
}

impl OriginEventContext<()> {
    pub fn new<'a, T, U>(
        source_cb: impl Fn() -> &'a T,
    ) -> impl Future<Output = OriginEventContext<T>> + 'static
    where
        T: OriginEventSource<U> + 'static,
    {
        let Ok(Some(origin_event_collector)) =
            ActiveOriginContext::get_value(&ORIGIN_EVENT_COLLECTOR)
        else {
            return ready(OriginEventContext {
                inner: None,
                _phantom: PhantomData,
            })
            .left_future();
        };
        let event = source_cb().as_event();
        async move {
            let parent_event_id = origin_event_collector
                .publish_origin_event(event, None)
                .await;
            OriginEventContext {
                inner: Some(OriginEventContextImpl {
                    origin_event_collector,
                    parent_event_id,
                }),
                _phantom: PhantomData,
            }
        }
        .right_future()
    }
}
impl<U> OriginEventContext<U> {
    pub fn emit<'a, T, F>(
        &self,
        event_cb: F,
    ) -> impl Future<Output = ()> + Send + use<'_, 'a, T, F, U>
    where
        T: OriginEventSource<U> + 'a,
        F: Fn() -> &'a T,
    {
        let Some(inner) = &self.inner else {
            return ready(()).left_future();
        };
        let v = (event_cb)();
        v.publish(inner).right_future()
    }

    pub fn wrap_stream<'a, O, S>(&self, stream: S) -> Pin<Box<dyn Stream<Item = O> + Send + 'a>>
    where
        U: Send + 'a,
        O: OriginEventSource<U> + Send + 'a,
        S: Stream<Item = O> + Send + 'a,
    {
        if self.inner.is_none() {
            return Box::pin(stream);
        }
        let ctx = self.clone();
        Box::pin(stream.then(move |item| {
            let ctx = ctx.clone();
            async move {
                ctx.emit(|| &item).await;
                item
            }
        }))
    }
}

#[derive(Clone)]
pub struct OriginEventContextImpl {
    origin_event_collector: Arc<OriginEventCollector>,
    parent_event_id: Uuid,
}

pub trait OriginEventSource<Source>: Sized {
    fn as_event(&self) -> Event;

    fn publish<'a>(&self, ctx: &'a OriginEventContextImpl) -> impl Future<Output = ()> + Send + 'a {
        let event = self.as_event();
        ctx.origin_event_collector
            .publish_origin_event(event, Some(ctx.parent_event_id))
            // We don't need the Uuid here.
            .map(|_| ())
    }
}

impl<'a, T, U> OriginEventSource<U> for &'a Response<T>
where
    T: OriginEventSource<U> + 'a,
{
    fn as_event(&self) -> Event {
        self.get_ref().as_event()
    }
}

impl<T, U> OriginEventSource<U> for Result<Response<T>, TonicStatus>
where
    T: OriginEventSource<U>,
    TonicStatus: OriginEventSource<U>,
{
    fn as_event(&self) -> Event {
        match self {
            Ok(v) => v.as_event(),
            Err(v) => v.as_event(),
        }
    }
}

// -- GetCapabilities --

impl OriginEventSource<()> for GetCapabilitiesRequest {
    fn as_event(&self) -> Event {
        Event::GetCapabilitiesRequest(self.clone())
    }
}
impl OriginEventSource<GetCapabilitiesRequest> for ServerCapabilities {
    fn as_event(&self) -> Event {
        Event::GetCapabilitiesResponse(self.clone())
    }
}

// -- GetActionResult --

impl OriginEventSource<()> for GetActionResultRequest {
    fn as_event(&self) -> Event {
        Event::GetActionResultRequest(self.clone())
    }
}
impl OriginEventSource<GetActionResultRequest> for ActionResult {
    fn as_event(&self) -> Event {
        Event::GetActionResultResponse(self.clone())
    }
}
impl OriginEventSource<GetActionResultRequest> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::GetActionResultError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}

// -- UpdateActionResult --

impl OriginEventSource<()> for UpdateActionResultRequest {
    fn as_event(&self) -> Event {
        Event::UpdateActionResultRequest(self.clone())
    }
}
impl OriginEventSource<UpdateActionResultRequest> for ActionResult {
    fn as_event(&self) -> Event {
        Event::UpdateActionResultResponse(self.clone())
    }
}
impl OriginEventSource<UpdateActionResultRequest> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::UpdateActionResultError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}

// -- FindMissingBlobs --

impl OriginEventSource<()> for FindMissingBlobsRequest {
    fn as_event(&self) -> Event {
        Event::FindMissingBlobsRequest(self.clone())
    }
}
impl OriginEventSource<FindMissingBlobsRequest> for FindMissingBlobsResponse {
    fn as_event(&self) -> Event {
        Event::FindMissingBlobsResponse(self.clone())
    }
}
impl OriginEventSource<FindMissingBlobsRequest> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::FindMissingBlobsError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}

// -- BatchReadBlobs --

impl OriginEventSource<()> for BatchReadBlobsRequest {
    fn as_event(&self) -> Event {
        Event::BatchReadBlobsRequest(self.clone())
    }
}
impl OriginEventSource<BatchReadBlobsRequest> for BatchReadBlobsResponse {
    fn as_event(&self) -> Event {
        Event::BatchReadBlobsResponse(BatchReadBlobsResponseOverride {
            responses: self
                .responses
                .iter()
                .map(|v| batch_read_blobs_response_override::Response {
                    digest: v.digest.clone(),
                    compressor: v.compressor,
                    status: v.status.clone(),
                    data_len: u64::try_from(v.data.len()).unwrap_or_default(),
                })
                .collect(),
        })
    }
}
impl OriginEventSource<BatchReadBlobsRequest> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::BatchReadBlobsError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}

// -- BatchUpdateBlobs --

impl OriginEventSource<()> for BatchUpdateBlobsRequest {
    fn as_event(&self) -> Event {
        Event::BatchUpdateBlobsRequest(BatchUpdateBlobsRequestOverride {
            instance_name: self.instance_name.clone(),
            requests: self
                .requests
                .iter()
                .map(|v| batch_update_blobs_request_override::Request {
                    digest: v.digest.clone(),
                    compressor: v.compressor,
                    data_len: u64::try_from(v.data.len()).unwrap_or_default(),
                })
                .collect(),
            digest_function: self.digest_function,
        })
    }
}
impl OriginEventSource<BatchUpdateBlobsRequest> for BatchUpdateBlobsResponse {
    fn as_event(&self) -> Event {
        Event::BatchUpdateBlobsResponse(self.clone())
    }
}
impl OriginEventSource<BatchUpdateBlobsRequest> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::BatchUpdateBlobsError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}

// -- GetTree --

impl OriginEventSource<()> for GetTreeRequest {
    fn as_event(&self) -> Event {
        Event::GetTreeRequest(self.clone())
    }
}
impl OriginEventSource<GetTreeRequest> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::GetTreeError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}
impl OriginEventSource<GetTreeRequest>
    for Pin<Box<dyn Stream<Item = Result<GetTreeResponse, TonicStatus>> + Send + '_>>
{
    fn as_event(&self) -> Event {
        Event::ReadResponse(())
    }
}
impl OriginEventSource<GetTreeRequest> for Result<GetTreeResponse, TonicStatus> {
    fn as_event(&self) -> Event {
        Event::GetTreeStreamResponse(GetTreeStreamResponse {
            response: Some(match self {
                Ok(v) => get_tree_stream_response::Response::Success(v.clone()),
                Err(v) => get_tree_stream_response::Response::Error(Status {
                    code: v.code().into(),
                    message: v.message().into(),
                    details: Vec::new(),
                }),
            }),
        })
    }
}

// -- ReadRequest --

impl OriginEventSource<()> for ReadRequest {
    fn as_event(&self) -> Event {
        Event::ReadRequest(self.clone())
    }
}
impl OriginEventSource<ReadRequest> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::ReadError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}
impl OriginEventSource<ReadRequest>
    for Pin<Box<dyn Stream<Item = Result<ReadResponse, TonicStatus>> + Send + '_>>
{
    fn as_event(&self) -> Event {
        Event::ReadResponse(())
    }
}
impl OriginEventSource<ReadRequest> for Result<ReadResponse, TonicStatus> {
    fn as_event(&self) -> Event {
        Event::ReadStreamResponse(ReadStreamResponse {
            response: Some(match self {
                Ok(v) => read_stream_response::Response::Success(
                    u64::try_from(v.data.len()).unwrap_or_default(),
                ),
                Err(v) => read_stream_response::Response::Error(Status {
                    code: v.code().into(),
                    message: v.message().into(),
                    details: Vec::new(),
                }),
            }),
        })
    }
}

// -- WriteRequest --

impl OriginEventSource<()> for Streaming<WriteRequest> {
    fn as_event(&self) -> Event {
        Event::WriteRequest(())
    }
}
impl OriginEventSource<Streaming<WriteRequest>> for WriteResponse {
    fn as_event(&self) -> Event {
        Event::WriteResponse(*self)
    }
}
impl OriginEventSource<Streaming<WriteRequest>> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::WriteError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}
impl OriginEventSource<Streaming<WriteRequest>> for Result<WriteRequest, TonicStatus> {
    fn as_event(&self) -> Event {
        Event::WriteStreamRequest(WriteStreamRequest {
            response: Some(match self {
                Ok(v) => write_stream_request::Response::Success(WriteRequestOverride {
                    resource_name: v.resource_name.clone(),
                    write_offset: v.write_offset,
                    finish_write: v.finish_write,
                    data_len: u64::try_from(v.data.len()).unwrap_or_default(),
                }),
                Err(v) => write_stream_request::Response::Error(Status {
                    code: v.code().into(),
                    message: v.message().into(),
                    details: Vec::new(),
                }),
            }),
        })
    }
}

// -- QueryWriteStatus --

impl OriginEventSource<()> for QueryWriteStatusRequest {
    fn as_event(&self) -> Event {
        Event::QueryWriteStatusRequest(self.clone())
    }
}
impl OriginEventSource<QueryWriteStatusRequest> for QueryWriteStatusResponse {
    fn as_event(&self) -> Event {
        Event::QueryWriteStatusResponse(*self)
    }
}
impl OriginEventSource<QueryWriteStatusRequest> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::QueryWriteStatusError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}

// -- ExecuteRequest --

impl OriginEventSource<()> for ExecuteRequest {
    fn as_event(&self) -> Event {
        Event::ExecuteRequest(self.clone())
    }
}
impl OriginEventSource<ExecuteRequest>
    for Pin<Box<dyn Stream<Item = Result<Operation, TonicStatus>> + Send + '_>>
{
    fn as_event(&self) -> Event {
        Event::ExecuteResponse(())
    }
}
impl OriginEventSource<ExecuteRequest> for QueryWriteStatusResponse {
    fn as_event(&self) -> Event {
        Event::QueryWriteStatusResponse(*self)
    }
}
impl OriginEventSource<ExecuteRequest> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::QueryWriteStatusError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}
impl OriginEventSource<ExecuteRequest> for Result<Operation, TonicStatus> {
    fn as_event(&self) -> Event {
        Event::ExecuteStreamResponse(ExecuteStreamResponse {
            response: Some(match self {
                Ok(v) => execute_stream_response::Response::Success(v.clone()),
                Err(v) => execute_stream_response::Response::Error(Status {
                    code: v.code().into(),
                    message: v.message().into(),
                    details: Vec::new(),
                }),
            }),
        })
    }
}

// -- WaitExecution --

impl OriginEventSource<()> for WaitExecutionRequest {
    fn as_event(&self) -> Event {
        Event::WaitExecutionRequest(self.clone())
    }
}
impl OriginEventSource<WaitExecutionRequest>
    for Pin<Box<dyn Stream<Item = Result<Operation, TonicStatus>> + Send + '_>>
{
    fn as_event(&self) -> Event {
        Event::ExecuteResponse(())
    }
}
impl OriginEventSource<WaitExecutionRequest> for QueryWriteStatusResponse {
    fn as_event(&self) -> Event {
        Event::QueryWriteStatusResponse(*self)
    }
}
impl OriginEventSource<WaitExecutionRequest> for TonicStatus {
    fn as_event(&self) -> Event {
        Event::QueryWriteStatusError(Status {
            code: self.code().into(),
            message: self.message().into(),
            details: Vec::new(),
        })
    }
}
impl OriginEventSource<WaitExecutionRequest> for Result<Operation, TonicStatus> {
    fn as_event(&self) -> Event {
        Event::WaitExecuteStreamResponse(ExecuteStreamResponse {
            response: Some(match self {
                Ok(v) => execute_stream_response::Response::Success(v.clone()),
                Err(v) => execute_stream_response::Response::Error(Status {
                    code: v.code().into(),
                    message: v.message().into(),
                    details: Vec::new(),
                }),
            }),
        })
    }
}
