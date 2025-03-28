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

use std::fmt::Debug;

use async_trait::async_trait;
use nativelink_error::Error;
use nativelink_util::buf_channel::DropCloserReadHalf;

use crate::gcs_client::types::{GcsObject, ObjectPath};

/// A trait that defines the required GCS operations.
/// This abstraction allows for easier testing by mocking GCS responses.
#[async_trait]
pub trait GcsOperations: Send + Sync + Debug {
    /// Read metadata for a GCS object
    async fn read_object_metadata(&self, object: ObjectPath) -> Result<Option<GcsObject>, Error>;

    /// Read the content of a GCS object, optionally with a range
    async fn read_object_content(
        &self,
        object: ObjectPath,
        start: i64,
        end: Option<i64>,
    ) -> Result<Vec<u8>, Error>;

    /// Write object with simple upload (for smaller objects)
    async fn write_object(&self, object: &ObjectPath, content: Vec<u8>) -> Result<(), Error>;

    /// Start a resumable write operation and return the upload URL
    async fn start_resumable_write(&self, object: &ObjectPath) -> Result<String, Error>;

    /// Upload a chunk of data in a resumable upload session
    async fn upload_chunk(
        &self,
        upload_url: &str,
        object: &ObjectPath,
        data: Vec<u8>,
        offset: i64,
        end_offset: i64,
        is_final: bool,
    ) -> Result<(), Error>;

    /// Complete high-level operation to upload data from a reader
    async fn upload_from_reader(
        &self,
        object: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        upload_id: &str,
        max_size: i64,
    ) -> Result<(), Error>;

    /// Check if an object exists
    async fn object_exists(&self, object: &ObjectPath) -> Result<bool, Error>;
}
