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

use core::fmt::Debug;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::Stream;
use nativelink_error::{Code, Error, make_err};
use nativelink_util::buf_channel::DropCloserReadHalf;
use tokio::sync::RwLock;

use crate::gcs_client::client::GcsOperations;
use crate::gcs_client::types::{GcsObject, ObjectPath, Timestamp};

/// A mock implementation of `GcsOperations` for testing
#[derive(Debug)]
pub struct MockGcsOperations {
    // Storage of mock objects with their content
    objects: RwLock<HashMap<String, MockObject>>,
    // Flag to simulate failures
    should_fail: AtomicBool,
    // Flag to simulate specific failure modes
    failure_mode: RwLock<FailureMode>,
    // Counter for operation calls
    call_counts: CallCounts,
    // For capturing requests to verify correct parameter passing
    requests: RwLock<Vec<MockRequest>>,
}

#[derive(Debug, Clone)]
struct MockObject {
    metadata: GcsObject,
    content: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct CallCounts {
    pub metadata_calls: AtomicUsize,
    pub read_calls: AtomicUsize,
    pub write_calls: AtomicUsize,
    pub start_resumable_calls: AtomicUsize,
    pub upload_chunk_calls: AtomicUsize,
    pub upload_from_reader_calls: AtomicUsize,
    pub object_exists_calls: AtomicUsize,
}

impl Clone for CallCounts {
    fn clone(&self) -> Self {
        Self {
            metadata_calls: AtomicUsize::new(self.metadata_calls.load(Ordering::Relaxed)),
            read_calls: AtomicUsize::new(self.read_calls.load(Ordering::Relaxed)),
            write_calls: AtomicUsize::new(self.write_calls.load(Ordering::Relaxed)),
            start_resumable_calls: AtomicUsize::new(
                self.start_resumable_calls.load(Ordering::Relaxed),
            ),
            upload_chunk_calls: AtomicUsize::new(self.upload_chunk_calls.load(Ordering::Relaxed)),
            upload_from_reader_calls: AtomicUsize::new(
                self.upload_from_reader_calls.load(Ordering::Relaxed),
            ),
            object_exists_calls: AtomicUsize::new(self.object_exists_calls.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Debug, Clone)]
pub enum MockRequest {
    ReadMetadata {
        object_path: ObjectPath,
    },
    ReadContent {
        object_path: ObjectPath,
        start: u64,
        end: Option<u64>,
    },
    Write {
        object_path: ObjectPath,
        content_len: usize,
    },
    UploadFromReader {
        object_path: ObjectPath,
    },
    ObjectExists {
        object_path: ObjectPath,
    },
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureMode {
    #[default]
    None,
    NotFound,
    NetworkError,
    Unauthorized,
    ServerError,
}

impl MockGcsOperations {
    /// Create a new empty mock GCS operations
    pub fn new() -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
            should_fail: AtomicBool::new(false),
            failure_mode: RwLock::new(FailureMode::None),
            call_counts: CallCounts::default(),
            requests: RwLock::new(Vec::new()),
        }
    }

    /// Set whether operations should fail or not
    pub fn set_should_fail(&self, should_fail: bool) {
        self.should_fail.store(should_fail, Ordering::Relaxed);
    }

    /// Set the specific failure mode to simulate
    pub async fn set_failure_mode(&self, mode: FailureMode) {
        *self.failure_mode.write().await = mode;
    }

    /// Add a mock object to the store
    pub async fn add_object(&self, path: &ObjectPath, content: Vec<u8>) {
        let object_key = self.get_object_key(path);

        // Get current timestamp
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let metadata = GcsObject {
            name: path.path.clone(),
            bucket: path.bucket.clone(),
            size: content.len() as i64,
            content_type: "application/octet-stream".into(),
            update_time: Some(Timestamp {
                seconds: now,
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
    pub fn get_call_counts(&self) -> CallCounts {
        self.call_counts.clone()
    }

    /// Reset all operation counters
    pub async fn reset_counters(&self) {
        self.call_counts.metadata_calls.store(0, Ordering::Relaxed);
        self.call_counts.read_calls.store(0, Ordering::Relaxed);
        self.call_counts.write_calls.store(0, Ordering::Relaxed);
        self.call_counts
            .start_resumable_calls
            .store(0, Ordering::Relaxed);
        self.call_counts
            .upload_chunk_calls
            .store(0, Ordering::Relaxed);
        self.call_counts
            .upload_from_reader_calls
            .store(0, Ordering::Relaxed);
        self.call_counts
            .object_exists_calls
            .store(0, Ordering::Relaxed);
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
        if self.should_fail.load(Ordering::Relaxed) {
            let value = *self.failure_mode.read().await;
            match value {
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
            content_type: "application/octet-stream".into(),
            update_time: Some(Timestamp {
                seconds: timestamp,
                nanos: 0,
            }),
        };

        let mock_object = MockObject { metadata, content };
        self.objects.write().await.insert(object_key, mock_object);
    }
}

impl GcsOperations for MockGcsOperations {
    async fn read_object_metadata(
        &self,
        object_path: &ObjectPath,
    ) -> Result<Option<GcsObject>, Error> {
        self.call_counts
            .metadata_calls
            .fetch_add(1, Ordering::Relaxed);
        self.requests.write().await.push(MockRequest::ReadMetadata {
            object_path: object_path.clone(),
        });

        self.handle_failure().await?;

        let object_key = self.get_object_key(object_path);
        let objects = self.objects.read().await;

        Ok(objects.get(&object_key).map(|obj| obj.metadata.clone()))
    }

    async fn read_object_content(
        &self,
        object_path: &ObjectPath,
        start: u64,
        end: Option<u64>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>>, Error> {
        struct OnceStream {
            content: Option<Bytes>,
        }
        impl Stream for OnceStream {
            type Item = Result<Bytes, Error>;

            fn poll_next(
                mut self: Pin<&mut Self>,
                _cx: &mut core::task::Context<'_>,
            ) -> core::task::Poll<Option<Self::Item>> {
                core::task::Poll::Ready(self.content.take().map(Ok))
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                match &self.content {
                    Some(bytes) => (bytes.len(), Some(bytes.len())),
                    None => (0, Some(0)),
                }
            }
        }

        self.call_counts.read_calls.fetch_add(1, Ordering::Relaxed);
        self.requests.write().await.push(MockRequest::ReadContent {
            object_path: object_path.clone(),
            start,
            end,
        });

        self.handle_failure().await?;

        let object_key = self.get_object_key(object_path);
        let objects = self.objects.read().await;

        if let Some(obj) = objects.get(&object_key) {
            let content = &obj.content;

            let start_idx = usize::try_from(start).unwrap_or(usize::MAX);
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
                core::cmp::min(usize::try_from(e).unwrap_or(usize::MAX), content.len())
            } else {
                content.len()
            };

            Ok(Box::pin(OnceStream {
                content: Some(Bytes::copy_from_slice(&content[start_idx..end_idx])),
            }))
        } else {
            Err(make_err!(Code::NotFound, "Object not found"))
        }
    }

    async fn upload_from_reader(
        &self,
        object_path: &ObjectPath,
        mut reader: DropCloserReadHalf,
    ) -> Result<(), Error> {
        self.call_counts
            .upload_from_reader_calls
            .fetch_add(1, Ordering::Relaxed);
        self.requests
            .write()
            .await
            .push(MockRequest::UploadFromReader {
                object_path: object_path.clone(),
            });

        self.handle_failure().await?;

        // Read all data from the reader
        let mut buffer = Vec::new();

        loop {
            let chunk = reader.recv().await?;
            if chunk.is_empty() {
                break;
            }
            buffer.extend_from_slice(&chunk);
        }

        self.call_counts.write_calls.fetch_add(1, Ordering::Relaxed);
        self.requests.write().await.push(MockRequest::Write {
            object_path: object_path.clone(),
            content_len: buffer.len(),
        });

        self.handle_failure().await?;
        self.add_object(object_path, buffer).await;
        Ok(())
    }

    async fn object_exists(&self, object_path: &ObjectPath) -> Result<bool, Error> {
        self.call_counts
            .object_exists_calls
            .fetch_add(1, Ordering::Relaxed);
        self.requests.write().await.push(MockRequest::ObjectExists {
            object_path: object_path.clone(),
        });

        self.handle_failure().await?;

        let object_key = self.get_object_key(object_path);
        let objects = self.objects.read().await;

        Ok(objects.contains_key(&object_key))
    }
}

impl Default for MockGcsOperations {
    fn default() -> Self {
        Self::new()
    }
}
