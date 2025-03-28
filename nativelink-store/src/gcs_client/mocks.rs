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

use std::collections::HashMap;
use std::fmt::Debug;

use async_trait::async_trait;
use nativelink_error::{make_err, Code, Error};
use nativelink_util::buf_channel::DropCloserReadHalf;
use tokio::sync::RwLock;

use crate::gcs_client::operations::GcsOperations;
use crate::gcs_client::types::{GcsObject, ObjectPath, Timestamp};

/// A mock implementation of `GcsOperations` for testing
#[derive(Debug)]
pub struct MockGcsOperations {
    // Storage of mock objects with their content
    objects: RwLock<HashMap<String, MockObject>>,
    // Flag to simulate failures
    should_fail: RwLock<bool>,
    // Flag to simulate specific failure modes
    failure_mode: RwLock<FailureMode>,
    // Counter for operation calls
    call_counts: RwLock<CallCounts>,
    // For capturing requests to verify correct parameter passing
    requests: RwLock<Vec<MockRequest>>,
}

#[derive(Debug, Clone)]
struct MockObject {
    metadata: GcsObject,
    content: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct CallCounts {
    pub metadata_calls: usize,
    pub read_calls: usize,
    pub write_calls: usize,
    pub start_resumable_calls: usize,
    pub upload_chunk_calls: usize,
    pub upload_from_reader_calls: usize,
    pub object_exists_calls: usize,
}

#[derive(Debug, Clone)]
pub enum MockRequest {
    ReadMetadata {
        object_path: ObjectPath,
    },
    ReadContent {
        object_path: ObjectPath,
        start: i64,
        end: Option<i64>,
    },
    Write {
        object_path: ObjectPath,
        content_len: usize,
    },
    StartResumable {
        object_path: ObjectPath,
    },
    UploadChunk {
        upload_url: String,
        object_path: ObjectPath,
        data_len: usize,
        offset: i64,
        end_offset: i64,
        is_final: bool,
    },
    UploadFromReader {
        object_path: ObjectPath,
        upload_id: String,
        max_size: i64,
    },
    ObjectExists {
        object_path: ObjectPath,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FailureMode {
    None,
    NotFound,
    NetworkError,
    Unauthorized,
    ServerError,
}

impl Default for FailureMode {
    fn default() -> Self {
        Self::None
    }
}

impl MockGcsOperations {
    /// Create a new empty mock GCS operations
    pub fn new() -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
            should_fail: RwLock::new(false),
            failure_mode: RwLock::new(FailureMode::None),
            call_counts: RwLock::new(CallCounts::default()),
            requests: RwLock::new(Vec::new()),
        }
    }

    /// Set whether operations should fail or not
    pub async fn set_should_fail(&self, should_fail: bool) {
        *self.should_fail.write().await = should_fail;
    }

    /// Set the specific failure mode to simulate
    pub async fn set_failure_mode(&self, mode: FailureMode) {
        *self.failure_mode.write().await = mode;
    }

    /// Add a mock object to the store
    pub async fn add_object(&self, path: &ObjectPath, content: Vec<u8>) {
        let object_key = self.get_object_key(path);

        let metadata = GcsObject {
            name: path.path.clone(),
            bucket: path.bucket.clone(),
            size: content.len() as i64,
            content_type: "application/octet-stream".to_string(),
            update_time: Some(Timestamp {
                seconds: chrono::Utc::now().timestamp(),
                nanos: 0,
            }),
        };

        let mock_object = MockObject { metadata, content };

        self.objects.write().await.insert(object_key, mock_object);
    }

    /// Remove a mock object from the store
    pub async fn remove_object(&self, path: &ObjectPath) -> bool {
        let object_key = self.get_object_key(path);
        self.objects.write().await.remove(&object_key).is_some()
    }

    /// Get the count of all operation calls
    pub async fn get_call_counts(&self) -> CallCounts {
        self.call_counts.read().await.clone()
    }

    /// Reset all operation counters
    pub async fn reset_counters(&self) {
        *self.call_counts.write().await = CallCounts::default();
        self.requests.write().await.clear();
    }

    /// Clear all objects from the store
    pub async fn clear_objects(&self) {
        self.objects.write().await.clear();
    }

    /// Get all recorded requests
    pub async fn get_requests(&self) -> Vec<MockRequest> {
        self.requests.read().await.clone()
    }

    /// Helper method to create a consistent key for objects
    fn get_object_key(&self, path: &ObjectPath) -> String {
        format!("{}/{}", path.bucket, path.path)
    }

    /// Helper method to handle failures based on current settings
    async fn handle_failure(&self) -> Result<(), Error> {
        // Add an explicit read lock on should_fail to ensure we get the latest value
        if *self.should_fail.read().await {
            match *self.failure_mode.read().await {
                FailureMode::None => Err(make_err!(Code::Internal, "Simulated generic failure")),
                FailureMode::NotFound => {
                    Err(make_err!(Code::NotFound, "Simulated not found error"))
                }
                FailureMode::NetworkError => {
                    Err(make_err!(Code::Unavailable, "Simulated network error"))
                }
                FailureMode::Unauthorized => Err(make_err!(
                    Code::Unauthenticated,
                    "Simulated authentication failure"
                )),
                FailureMode::ServerError => {
                    Err(make_err!(Code::Internal, "Simulated server error"))
                }
            }
        } else {
            Ok(())
        }
    }

    /// Add a mock object with a specific timestamp
    pub async fn add_object_with_timestamp(
        &self,
        path: &ObjectPath,
        content: Vec<u8>,
        timestamp: i64,
    ) {
        let object_key = self.get_object_key(path);

        let metadata = GcsObject {
            name: path.path.clone(),
            bucket: path.bucket.clone(),
            size: content.len() as i64,
            content_type: "application/octet-stream".to_string(),
            update_time: Some(Timestamp {
                seconds: timestamp,
                nanos: 0,
            }),
        };

        let mock_object = MockObject { metadata, content };
        self.objects.write().await.insert(object_key, mock_object);
    }
}

#[async_trait]
impl GcsOperations for MockGcsOperations {
    async fn read_object_metadata(&self, object: ObjectPath) -> Result<Option<GcsObject>, Error> {
        self.call_counts.write().await.metadata_calls += 1;
        self.requests.write().await.push(MockRequest::ReadMetadata {
            object_path: object.clone(),
        });

        self.handle_failure().await?;

        let object_key = self.get_object_key(&object);
        let objects = self.objects.read().await;

        Ok(objects.get(&object_key).map(|obj| obj.metadata.clone()))
    }

    async fn read_object_content(
        &self,
        object: ObjectPath,
        start: i64,
        end: Option<i64>,
    ) -> Result<Vec<u8>, Error> {
        self.call_counts.write().await.read_calls += 1;
        self.requests.write().await.push(MockRequest::ReadContent {
            object_path: object.clone(),
            start,
            end,
        });

        self.handle_failure().await?;

        let object_key = self.get_object_key(&object);
        let objects = self.objects.read().await;

        if let Some(obj) = objects.get(&object_key) {
            let content = &obj.content;
            if start < 0 {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "Start index {} must be non-negative",
                    start
                ));
            }

            let start_idx = start as usize;
            if start_idx > content.len() {
                return Err(make_err!(
                    Code::OutOfRange,
                    "Start index {} exceeds content length {}",
                    start,
                    content.len()
                ));
            }

            // Calculate end index with validation
            let end_idx = if let Some(e) = end {
                if e < start {
                    return Err(make_err!(
                        Code::InvalidArgument,
                        "End index {} must be greater than or equal to start index {}",
                        e,
                        start
                    ));
                }
                std::cmp::min(e as usize, content.len())
            } else {
                content.len()
            };

            Ok(content[start_idx..end_idx].to_vec())
        } else {
            Err(make_err!(Code::NotFound, "Object not found"))
        }
    }

    async fn write_object(&self, object: &ObjectPath, content: Vec<u8>) -> Result<(), Error> {
        self.call_counts.write().await.write_calls += 1;
        self.requests.write().await.push(MockRequest::Write {
            object_path: object.clone(),
            content_len: content.len(),
        });

        self.handle_failure().await?;
        self.add_object(object, content).await;
        Ok(())
    }

    async fn start_resumable_write(&self, object: &ObjectPath) -> Result<String, Error> {
        self.call_counts.write().await.start_resumable_calls += 1;
        self.requests
            .write()
            .await
            .push(MockRequest::StartResumable {
                object_path: object.clone(),
            });

        self.handle_failure().await?;
        let upload_id = format!("mock-upload-{}-{}", object.bucket, object.path);
        Ok(upload_id)
    }

    async fn upload_chunk(
        &self,
        upload_url: &str,
        object: &ObjectPath,
        data: Vec<u8>,
        offset: i64,
        end_offset: i64,
        is_final: bool,
    ) -> Result<(), Error> {
        self.call_counts.write().await.upload_chunk_calls += 1;
        self.requests.write().await.push(MockRequest::UploadChunk {
            upload_url: upload_url.to_string(),
            object_path: object.clone(),
            data_len: data.len(),
            offset,
            end_offset,
            is_final,
        });

        self.handle_failure().await?;

        let object_key = self.get_object_key(object);
        let mut objects = self.objects.write().await;

        // Get or create the object
        let mock_object = if let Some(obj) = objects.get_mut(&object_key) {
            obj
        } else {
            let new_obj = MockObject {
                metadata: GcsObject {
                    name: object.path.clone(),
                    bucket: object.bucket.clone(),
                    size: 0,
                    content_type: "application/octet-stream".to_string(),
                    update_time: Some(Timestamp {
                        seconds: chrono::Utc::now().timestamp(),
                        nanos: 0,
                    }),
                },
                content: Vec::new(),
            };
            objects.insert(object_key.clone(), new_obj);
            objects.get_mut(&object_key).unwrap()
        };

        // Handle the chunk data
        let offset_usize = offset as usize;
        if mock_object.content.len() < offset_usize + data.len() {
            mock_object.content.resize(offset_usize + data.len(), 0);
        }

        if !data.is_empty() {
            mock_object.content[offset_usize..offset_usize + data.len()].copy_from_slice(&data);
        }

        // Update metadata if this is the final chunk
        if is_final {
            mock_object.metadata.size = mock_object.content.len() as i64;
            mock_object.metadata.update_time = Some(Timestamp {
                seconds: chrono::Utc::now().timestamp(),
                nanos: 0,
            });
        }

        Ok(())
    }

    async fn upload_from_reader(
        &self,
        object: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        upload_id: &str,
        max_size: i64,
    ) -> Result<(), Error> {
        self.call_counts.write().await.upload_from_reader_calls += 1;
        self.requests
            .write()
            .await
            .push(MockRequest::UploadFromReader {
                object_path: object.clone(),
                upload_id: upload_id.to_string(),
                max_size,
            });

        self.handle_failure().await?;

        // Read all data from the reader
        let mut buffer = Vec::new();
        let mut total_read = 0i64;

        while total_read < max_size {
            let to_read = std::cmp::min((max_size - total_read) as usize, 8192); // 8KB chunks
            let chunk = reader.consume(Some(to_read)).await?;

            if chunk.is_empty() {
                break;
            }

            buffer.extend_from_slice(&chunk);
            total_read += chunk.len() as i64;
        }

        self.write_object(object, buffer).await?;

        Ok(())
    }

    async fn object_exists(&self, object: &ObjectPath) -> Result<bool, Error> {
        self.call_counts.write().await.object_exists_calls += 1;
        self.requests.write().await.push(MockRequest::ObjectExists {
            object_path: object.clone(),
        });

        self.handle_failure().await?;

        let object_key = self.get_object_key(object);
        let objects = self.objects.read().await;

        Ok(objects.contains_key(&object_key))
    }
}

impl Default for MockGcsOperations {
    fn default() -> Self {
        Self::new()
    }
}
