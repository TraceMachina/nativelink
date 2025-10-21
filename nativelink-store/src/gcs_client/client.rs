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

use core::fmt::{Debug, Formatter};
use core::future::Future;
use core::pin::Pin;
use core::time::Duration;
use std::sync::Arc;

use bytes::Bytes;
use futures::stream::unfold;
use futures::{Stream, StreamExt};
use google_cloud_storage::client::{Storage, StorageControl};
use google_cloud_storage::model::Object;
use google_cloud_storage::model_ext::ReadRange;
use google_cloud_storage::streaming_source::StreamingSource;
use nativelink_config::stores::ExperimentalGcsSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_util::buf_channel::DropCloserReadHalf;
use tokio::sync::Semaphore;

use crate::gcs_client::Timestamp;
use crate::gcs_client::types::{CHUNK_SIZE, DEFAULT_CONCURRENT_UPLOADS, GcsObject, ObjectPath};

pub type ByteStream = Box<dyn Stream<Item = Result<Bytes, Error>> + Send>;

/// A trait that defines the required GCS operations.
/// This abstraction allows for easier testing by mocking GCS responses.
pub trait GcsOperations: Send + Sync + Debug {
    /// Read metadata for a GCS object
    fn read_object_metadata(
        &self,
        object: &ObjectPath,
    ) -> impl Future<Output = Result<Option<GcsObject>, Error>> + Send;

    /// Read the content of a GCS object, optionally with a range
    fn read_object_content(
        &self,
        object_path: &ObjectPath,
        start: u64,
        end: Option<u64>,
    ) -> impl Future<Output = Result<Pin<ByteStream>, Error>> + Send;

    /// Complete high-level operation to upload data from a reader
    fn upload_from_reader(
        &self,
        object_path: &ObjectPath,
        reader: DropCloserReadHalf,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// Check if an object exists
    fn object_exists(
        &self,
        object_path: &ObjectPath,
    ) -> impl Future<Output = Result<bool, Error>> + Send;
}

/// Main client for interacting with Google Cloud Storage
pub struct GcsClient {
    storage: Storage,
    control: StorageControl,
    semaphore: Arc<Semaphore>,
}

impl GcsClient {
    /// Create a new GCS client from the provided spec
    pub async fn new(spec: &ExperimentalGcsSpec) -> Result<Self, Error> {
        let credentials = google_cloud_auth::credentials::Builder::default()
            .build()
            .ok();

        // If authentication is required then error, otherwise use anonymous.
        let credentials = if spec.authentication_required {
            credentials.err_tip(|| "Authentication required and none found.")?
        } else {
            credentials.unwrap_or_else(|| {
                google_cloud_auth::credentials::anonymous::Builder::default().build()
            })
        };

        let resumable_chunk_size = spec.resumable_chunk_size.unwrap_or(CHUNK_SIZE);

        // Creating client with the configured authentication
        let storage = Storage::builder()
            .with_credentials(credentials.clone())
            .with_resumable_upload_buffer_size(resumable_chunk_size)
            .with_read_resume_policy(
                google_cloud_storage::read_resume_policy::LimitedAttemptCount::new(
                    google_cloud_storage::read_resume_policy::Recommended,
                    spec.common.retry.max_retries.try_into().map_err(|e| {
                        make_err!(Code::InvalidArgument, "Max retries is a bad value: {e:?}")
                    })?,
                ),
            )
            .with_retry_policy(google_cloud_gax::retry_policy::LimitedAttemptCount::new(
                spec.common.retry.max_retries.try_into().map_err(|e| {
                    make_err!(Code::InvalidArgument, "Max retries is a bad value: {e:?}")
                })?,
            ))
            .with_backoff_policy(
                google_cloud_gax::exponential_backoff::ExponentialBackoffBuilder::new()
                    .with_initial_delay(Duration::from_secs(1))
                    .with_scaling(spec.common.retry.delay)
                    .build()
                    .map_err(|e| {
                        make_err!(Code::Internal, "Unable to create backoff policy: {e:?}")
                    })?,
            )
            .build()
            .await
            .map_err(|e| make_err!(Code::Internal, "Unable to create GCS client: {e:?}"))?;
        let control = StorageControl::builder()
            .with_credentials(credentials)
            .with_retry_policy(google_cloud_gax::retry_policy::LimitedAttemptCount::new(
                spec.common.retry.max_retries.try_into().map_err(|e| {
                    make_err!(Code::InvalidArgument, "Max retries is a bad value: {e:?}")
                })?,
            ))
            .with_backoff_policy(
                google_cloud_gax::exponential_backoff::ExponentialBackoffBuilder::new()
                    .with_initial_delay(Duration::from_secs(1))
                    .with_scaling(spec.common.retry.delay)
                    .build()
                    .map_err(|e| {
                        make_err!(Code::Internal, "Unable to create backoff policy: {e:?}")
                    })?,
            )
            .build()
            .await
            .map_err(|e| make_err!(Code::Internal, "Unable to create GCS control client: {e:?}"))?;

        // Get max connections from config
        let max_connections = spec
            .common
            .multipart_max_concurrent_uploads
            .unwrap_or(DEFAULT_CONCURRENT_UPLOADS);

        Ok(Self {
            storage,
            control,
            semaphore: Arc::new(Semaphore::new(max_connections)),
        })
    }

    /// Generic method to execute operations with connection limiting
    async fn with_connection<F, Fut, T>(&self, operation: F) -> Result<T, Error>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T, Error>> + Send,
    {
        let permit =
            self.semaphore.acquire().await.map_err(|e| {
                make_err!(Code::Internal, "Failed to acquire connection permit: {}", e)
            })?;

        let result = operation().await;
        drop(permit);
        result
    }

    /// Convert GCS object to our internal representation
    fn convert_to_gcs_object(obj: Object) -> GcsObject {
        GcsObject {
            name: obj.name,
            bucket: obj.bucket,
            size: obj.size,
            content_type: obj.content_type,
            update_time: obj.update_time.map(|timestamp| Timestamp {
                seconds: timestamp.seconds(),
                nanos: timestamp.nanos(),
            }),
        }
    }

    /// Handle error from GCS operations
    #[allow(clippy::needless_pass_by_value)]
    fn handle_gcs_error(err: google_cloud_storage::Error) -> Error {
        let code = if err.is_timeout() || err.is_exhausted() {
            Code::DeadlineExceeded
        } else if let Some(code) = err.http_status_code() {
            match code {
                404 => Code::NotFound,
                401 | 403 => Code::PermissionDenied,
                408 | 429 => Code::ResourceExhausted,
                500..=599 => Code::Unavailable,
                _ => Code::Unknown,
            }
        } else {
            Code::Internal
        };
        make_err!(code, "GCS operation failed: {err}")
    }
}

impl Debug for GcsClient {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GcsClient")
            .field("max_connections", &self.semaphore.available_permits())
            .finish_non_exhaustive()
    }
}

impl GcsOperations for GcsClient {
    async fn read_object_metadata(
        &self,
        object_path: &ObjectPath,
    ) -> Result<Option<GcsObject>, Error> {
        self.with_connection(|| async {
            let result = self
                .control
                .get_object()
                .set_bucket(format!("projects/_/buckets/{}", object_path.bucket))
                .set_object(object_path.path.clone())
                .send()
                .await
                .map(Self::convert_to_gcs_object)
                .map_err(Self::handle_gcs_error);
            match result {
                Ok(obj) => Ok(Some(obj)),
                Err(err) if err.code == Code::NotFound => Ok(None),
                Err(err) => Err(err),
            }
        })
        .await
    }

    async fn read_object_content(
        &self,
        object_path: &ObjectPath,
        start: u64,
        end: Option<u64>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>>, Error> {
        let permit =
            self.semaphore.clone().acquire_owned().await.map_err(|e| {
                make_err!(Code::Internal, "Failed to acquire connection permit: {}", e)
            })?;

        let range = match (start, end) {
            (0, None) => ReadRange::all(),
            (0, Some(end)) => ReadRange::head(end),
            (start, None) => ReadRange::offset(start),
            (start, Some(end)) => ReadRange::segment(start, end - start),
        };
        let stream = self
            .storage
            .read_object(
                format!("projects/_/buckets/{}", object_path.bucket),
                &object_path.path,
            )
            .set_read_range(range)
            .send()
            .await
            .map_err(Self::handle_gcs_error)?;

        Ok(unfold((stream, permit), |(mut stream, permit)| async move {
            stream
                .next()
                .await
                .map(|result| (result.map_err(Self::handle_gcs_error), (stream, permit)))
        })
        .boxed())
    }

    async fn upload_from_reader(
        &self,
        object_path: &ObjectPath,
        reader: DropCloserReadHalf,
    ) -> Result<(), Error> {
        struct Adapter(DropCloserReadHalf);
        impl StreamingSource for Adapter {
            type Error = Error;

            async fn next(&mut self) -> Option<Result<Bytes, Self::Error>> {
                match self.0.recv().await {
                    Ok(bytes) => (bytes.is_empty()).then_some(bytes).map(Ok),
                    Err(err) => Some(Err(err)),
                }
            }
        }
        self.with_connection(|| async {
            self.storage
                .write_object(
                    format!("projects/_/buckets/{}", object_path.bucket),
                    &object_path.path,
                    Adapter(reader),
                )
                .send_buffered()
                .await
                .map_err(Self::handle_gcs_error)
                .map(|_| ())
        })
        .await
    }

    async fn object_exists(&self, object_path: &ObjectPath) -> Result<bool, Error> {
        let metadata = self.read_object_metadata(object_path).await?;
        Ok(metadata.is_some())
    }
}
