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

//! Inline definition of `google.rpc.PreconditionFailure`.
//!
//! Bazel's `RemoteSpawnRunner` reads this proto out of an
//! `ExecuteResponse.status.details` (or, for synchronous Execute
//! errors, the gRPC `Status.details` carried via
//! `grpc-status-details-bin`) of a `FAILED_PRECONDITION` response
//! and, for violations of type `MISSING`, automatically re-uploads
//! the named blobs and retries the Execute call. Without this detail
//! Bazel surfaces the failure as a hard build error.

/// `google.rpc.PreconditionFailure`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PreconditionFailure {
    #[prost(message, repeated, tag = "1")]
    pub violations: ::prost::alloc::vec::Vec<Violation>,
}

/// `google.rpc.PreconditionFailure.Violation`.
#[derive(Clone, Eq, PartialEq, ::prost::Message)]
pub struct Violation {
    #[prost(string, tag = "1")]
    pub r#type: ::prost::alloc::string::String,
    #[prost(string, tag = "2")]
    pub subject: ::prost::alloc::string::String,
    #[prost(string, tag = "3")]
    pub description: ::prost::alloc::string::String,
}

pub const TYPE_URL: &str = "type.googleapis.com/google.rpc.PreconditionFailure";
pub const VIOLATION_TYPE_MISSING: &str = "MISSING";
