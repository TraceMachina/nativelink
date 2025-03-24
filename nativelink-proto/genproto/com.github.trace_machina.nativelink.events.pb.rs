// Copyright 2022 The NativeLink Authors. All rights reserved.
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

// This file is @generated by prost-build.
/// / Same as build.bazel.remote.execution.v2.BatchUpdateBlobsRequest,
/// / but without the data field, and add a `data_len` field.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct BatchUpdateBlobsRequestOverride {
    #[prost(string, tag = "1")]
    pub instance_name: ::prost::alloc::string::String,
    #[prost(message, repeated, tag = "2")]
    pub requests: ::prost::alloc::vec::Vec<batch_update_blobs_request_override::Request>,
    #[prost(
        enumeration = "super::super::super::super::super::build::bazel::remote::execution::v2::digest_function::Value",
        tag = "5"
    )]
    pub digest_function: i32,
}
/// Nested message and enum types in `BatchUpdateBlobsRequestOverride`.
pub mod batch_update_blobs_request_override {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Request {
        #[prost(message, optional, tag = "1")]
        pub digest: ::core::option::Option<
            super::super::super::super::super::super::build::bazel::remote::execution::v2::Digest,
        >,
        #[prost(
            enumeration = "super::super::super::super::super::super::build::bazel::remote::execution::v2::compressor::Value",
            tag = "3"
        )]
        pub compressor: i32,
        /// Override/new field to track the length of the data.
        ///
        /// Using 15 to stay at 1 byte, but higher than 3.
        #[prost(uint64, tag = "15")]
        pub data_len: u64,
    }
}
/// / Same as build.bazel.remote.execution.v2.BatchReadBlobsResponse,
/// / but without the data field, and add a `data_len` field.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct BatchReadBlobsResponseOverride {
    #[prost(message, repeated, tag = "1")]
    pub responses: ::prost::alloc::vec::Vec<
        batch_read_blobs_response_override::Response,
    >,
}
/// Nested message and enum types in `BatchReadBlobsResponseOverride`.
pub mod batch_read_blobs_response_override {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Response {
        #[prost(message, optional, tag = "1")]
        pub digest: ::core::option::Option<
            super::super::super::super::super::super::build::bazel::remote::execution::v2::Digest,
        >,
        #[prost(
            enumeration = "super::super::super::super::super::super::build::bazel::remote::execution::v2::compressor::Value",
            tag = "4"
        )]
        pub compressor: i32,
        #[prost(message, optional, tag = "3")]
        pub status: ::core::option::Option<
            super::super::super::super::super::super::google::rpc::Status,
        >,
        /// Override/new field to track the length of the data.
        ///
        /// Using 15 to stay at 1 byte, but higher than 3.
        #[prost(uint64, tag = "15")]
        pub data_len: u64,
    }
}
/// / Same as google.bytestream.WriteRequest, but without the data field,
/// / and add a `data_len` field.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WriteRequestOverride {
    #[prost(string, tag = "1")]
    pub resource_name: ::prost::alloc::string::String,
    #[prost(int64, tag = "2")]
    pub write_offset: i64,
    #[prost(bool, tag = "3")]
    pub finish_write: bool,
    /// Override/new field to track the length of the data.
    ///
    /// Using 15 to stay at 1 byte, but higher than 3.
    #[prost(uint64, tag = "15")]
    pub data_len: u64,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RequestEvent {
    #[prost(
        oneof = "request_event::Event",
        tags = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14"
    )]
    pub event: ::core::option::Option<request_event::Event>,
}
/// Nested message and enum types in `RequestEvent`.
pub mod request_event {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Event {
        #[prost(message, tag = "1")]
        GetCapabilitiesRequest(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::GetCapabilitiesRequest,
        ),
        #[prost(message, tag = "2")]
        GetActionResultRequest(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::GetActionResultRequest,
        ),
        #[prost(message, tag = "3")]
        UpdateActionResultRequest(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::UpdateActionResultRequest,
        ),
        #[prost(message, tag = "4")]
        FindMissingBlobsRequest(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::FindMissingBlobsRequest,
        ),
        #[prost(message, tag = "5")]
        BatchReadBlobsRequest(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::BatchReadBlobsRequest,
        ),
        #[prost(message, tag = "6")]
        BatchUpdateBlobsRequest(super::BatchUpdateBlobsRequestOverride),
        #[prost(message, tag = "7")]
        GetTreeRequest(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::GetTreeRequest,
        ),
        #[prost(message, tag = "8")]
        ReadRequest(
            super::super::super::super::super::super::google::bytestream::ReadRequest,
        ),
        #[prost(message, tag = "9")]
        WriteRequest(()),
        #[prost(message, tag = "10")]
        QueryWriteStatusRequest(
            super::super::super::super::super::super::google::bytestream::QueryWriteStatusRequest,
        ),
        #[prost(message, tag = "11")]
        ExecuteRequest(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::ExecuteRequest,
        ),
        #[prost(message, tag = "12")]
        WaitExecutionRequest(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::WaitExecutionRequest,
        ),
        #[prost(message, tag = "13")]
        SchedulerStartExecute(super::super::remote_execution::StartExecute),
        #[prost(message, tag = "14")]
        FetchBlobRequest(
            super::super::super::super::super::super::build::bazel::remote::asset::v1::FetchBlobRequest,
        ),
    }
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ResponseEvent {
    #[prost(oneof = "response_event::Event", tags = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10")]
    pub event: ::core::option::Option<response_event::Event>,
}
/// Nested message and enum types in `ResponseEvent`.
pub mod response_event {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Event {
        #[prost(message, tag = "1")]
        Error(super::super::super::super::super::super::google::rpc::Status),
        #[prost(message, tag = "2")]
        ServerCapabilities(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::ServerCapabilities,
        ),
        #[prost(message, tag = "3")]
        ActionResult(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::ActionResult,
        ),
        #[prost(message, tag = "4")]
        FindMissingBlobsResponse(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::FindMissingBlobsResponse,
        ),
        #[prost(message, tag = "5")]
        BatchReadBlobsResponse(super::BatchReadBlobsResponseOverride),
        #[prost(message, tag = "6")]
        BatchUpdateBlobsResponse(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::BatchUpdateBlobsResponse,
        ),
        #[prost(message, tag = "7")]
        WriteResponse(
            super::super::super::super::super::super::google::bytestream::WriteResponse,
        ),
        #[prost(message, tag = "8")]
        QueryWriteStatusResponse(
            super::super::super::super::super::super::google::bytestream::QueryWriteStatusResponse,
        ),
        #[prost(message, tag = "9")]
        Empty(()),
        #[prost(message, tag = "10")]
        FetchBlobResponse(
            super::super::super::super::super::super::build::bazel::remote::asset::v1::FetchBlobResponse,
        ),
    }
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct StreamEvent {
    #[prost(oneof = "stream_event::Event", tags = "1, 2, 3, 4, 5, 6")]
    pub event: ::core::option::Option<stream_event::Event>,
}
/// Nested message and enum types in `StreamEvent`.
pub mod stream_event {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Event {
        #[prost(message, tag = "1")]
        Error(super::super::super::super::super::super::google::rpc::Status),
        #[prost(message, tag = "2")]
        GetTreeResponse(
            super::super::super::super::super::super::build::bazel::remote::execution::v2::GetTreeResponse,
        ),
        #[prost(uint64, tag = "3")]
        DataLength(u64),
        #[prost(message, tag = "4")]
        WriteRequest(super::WriteRequestOverride),
        #[prost(message, tag = "5")]
        Operation(
            super::super::super::super::super::super::google::longrunning::Operation,
        ),
        #[prost(message, tag = "6")]
        Closed(()),
    }
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Event {
    #[prost(oneof = "event::Event", tags = "1, 2, 3")]
    pub event: ::core::option::Option<event::Event>,
}
/// Nested message and enum types in `Event`.
pub mod event {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Event {
        #[prost(message, tag = "1")]
        Request(super::RequestEvent),
        #[prost(message, tag = "2")]
        Response(super::ResponseEvent),
        #[prost(message, tag = "3")]
        Stream(super::StreamEvent),
    }
}
/// / Nativelink event that has occurred.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct OriginEvent {
    /// / The version of this message.
    #[prost(uint32, tag = "1")]
    pub version: u32,
    /// / The event UUIDv6. This is a unique identifier for the event for the
    /// / server that generated the event.
    /// / Note: The timestamp of when the event occurred is encoded in the UUID.
    #[prost(string, tag = "2")]
    pub event_id: ::prost::alloc::string::String,
    /// / \[optional\] The parent event UUID. This is used to track the
    /// / parent event that generated this event. This is useful for
    /// / tracking the flow of events.
    #[prost(string, tag = "3")]
    pub parent_event_id: ::prost::alloc::string::String,
    /// / If the client is bazel, this is the meatadata that was sent with the
    /// / request. This is useful for tracking the flow of events.
    #[prost(message, optional, tag = "4")]
    pub bazel_request_metadata: ::core::option::Option<
        super::super::super::super::super::build::bazel::remote::execution::v2::RequestMetadata,
    >,
    /// / The identity header that generated the event. This will be populated with
    /// / the value of the specified by the `IdentityHeaderSpec::header_name`.
    #[prost(string, tag = "5")]
    pub identity: ::prost::alloc::string::String,
    /// / The event that occurred.
    #[prost(message, optional, tag = "6")]
    pub event: ::core::option::Option<Event>,
}
/// / Batch of events that have occurred.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct OriginEvents {
    #[prost(message, repeated, tag = "1")]
    pub events: ::prost::alloc::vec::Vec<OriginEvent>,
}
/// / Bep event that has occurred.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct BepEvent {
    /// / The version of this message.
    #[prost(uint32, tag = "1")]
    pub version: u32,
    /// / The identity header that generated the event. This will be populated
    /// / with the header value keyed by the specified by the
    /// / `IdentityHeaderSpec::header_name`.
    #[prost(string, tag = "2")]
    pub identity: ::prost::alloc::string::String,
    /// / The event that occurred.
    #[prost(oneof = "bep_event::Event", tags = "3, 4")]
    pub event: ::core::option::Option<bep_event::Event>,
}
/// Nested message and enum types in `BepEvent`.
pub mod bep_event {
    /// / The event that occurred.
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Event {
        #[prost(message, tag = "3")]
        LifecycleEvent(
            super::super::super::super::super::super::google::devtools::build::v1::PublishLifecycleEventRequest,
        ),
        #[prost(message, tag = "4")]
        BuildToolEvent(
            super::super::super::super::super::super::google::devtools::build::v1::PublishBuildToolEventStreamRequest,
        ),
    }
}
