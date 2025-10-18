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
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use futures::Stream;
use gcloud_auth::credentials::CredentialsFile;
use gcloud_storage::client::{Client, ClientConfig};
use gcloud_storage::http::Error as GcsError;
use gcloud_storage::http::objects::Object;
use gcloud_storage::http::objects::download::Range;
use gcloud_storage::http::objects::get::GetObjectRequest;
use gcloud_storage::http::objects::upload::{Media, UploadObjectRequest, UploadType};
use gcloud_storage::http::resumable_upload_client::{ChunkSize, UploadStatus};
use nativelink_config::stores::ExperimentalGcsSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_util::buf_channel::DropCloserReadHalf;
use rand::Rng;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::sleep;

use crate::gcs_client::types::{
    CHUNK_SIZE, DEFAULT_CONCURRENT_UPLOADS, DEFAULT_CONTENT_TYPE, GcsObject,
    INITIAL_UPLOAD_RETRY_DELAY_MS, MAX_UPLOAD_RETRIES, MAX_UPLOAD_RETRY_DELAY_MS, ObjectPath,
    SIMPLE_UPLOAD_THRESHOLD, Timestamp,
};

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
    ) -> impl Future<
        Output = Result<Box<dyn Stream<Item = Result<Bytes, Error>> + Send + Unpin>, Error>,
    > + Send;

    /// Write object with simple upload (for smaller objects)
    fn write_object(
        &self,
        object_path: &ObjectPath,
        content: Vec<u8>,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// Start a resumable write operation and return the upload URL
    fn start_resumable_write(
        &self,
        object_path: &ObjectPath,
    ) -> impl Future<Output = Result<String, Error>> + Send;

    /// Upload a chunk of data in a resumable upload session
    fn upload_chunk(
        &self,
        upload_url: &str,
        object_path: &ObjectPath,
        data: Bytes,
        offset: u64,
        end_offset: u64,
        total_size: Option<u64>,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// Complete high-level operation to upload data from a reader
    fn upload_from_reader(
        &self,
        object_path: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        upload_id: &str,
        max_size: u64,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Check if an object exists
    fn object_exists(
        &self,
        object_path: &ObjectPath,
    ) -> impl Future<Output = Result<bool, Error>> + Send;
}

/// Main client for interacting with Google Cloud Storage
pub struct GcsClient {
    client: Client,
    resumable_chunk_size: usize,
    semaphore: Arc<Semaphore>,
}

impl GcsClient {
    fn create_client_config(spec: &ExperimentalGcsSpec) -> Result<ClientConfig, Error> {
        let mut client_config = ClientConfig::default();
        let connect_timeout = if spec.connection_timeout_s > 0 {
            Duration::from_secs(spec.connection_timeout_s)
        } else {
            Duration::from_secs(3)
        };
        let read_timeout = if spec.read_timeout_s > 0 {
            Duration::from_secs(spec.read_timeout_s)
        } else {
            Duration::from_secs(3)
        };
        let client = reqwest::ClientBuilder::new()
            .connect_timeout(connect_timeout)
            .read_timeout(read_timeout)
            .build()
            .map_err(|e| make_err!(Code::Internal, "Unable to create GCS client: {e:?}"))?;
        let mid_client = reqwest_middleware::ClientBuilder::new(client).build();
        client_config.http = Some(mid_client);
        Ok(client_config)
    }

    /// Create a new GCS client from the provided spec
    pub async fn new(spec: &ExperimentalGcsSpec) -> Result<Self, Error> {
        // Attempt to get the authentication from a file with the environment
        // variable GOOGLE_APPLICATION_CREDENTIALS or directly from the
        // environment in variable GOOGLE_APPLICATION_CREDENTIALS_JSON.  If that
        // fails, attempt to get authentication from the environment.
        let maybe_client_config = match CredentialsFile::new().await {
            Ok(credentials) => {
                Self::create_client_config(spec)?
                    .with_credentials(credentials)
                    .await
            }
            Err(_) => Self::create_client_config(spec)?.with_auth().await,
        }
        .map_err(|e| {
            make_err!(
                Code::Internal,
                "Failed to create client config with credentials: {e:?}"
            )
        });

        // If authentication is required then error, otherwise use anonymous.
        let client_config = if spec.authentication_required {
            maybe_client_config.err_tip(|| "Authentication required and none found.")?
        } else {
            maybe_client_config
                .or_else(|_| Self::create_client_config(spec).map(ClientConfig::anonymous))?
        };

        // Creating client with the configured authentication
        let client = Client::new(client_config);
        let resumable_chunk_size = spec.resumable_chunk_size.unwrap_or(CHUNK_SIZE);

        // Get max connections from config
        let max_connections = spec
            .common
            .multipart_max_concurrent_uploads
            .unwrap_or(DEFAULT_CONCURRENT_UPLOADS);

        Ok(Self {
            client,
            resumable_chunk_size,
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
    fn convert_to_gcs_object(&self, obj: Object) -> GcsObject {
        let update_time = obj.updated.map(|dt| Timestamp {
            seconds: dt.unix_timestamp(),
            nanos: 0,
        });

        GcsObject {
            name: obj.name,
            bucket: obj.bucket,
            size: obj.size,
            content_type: obj
                .content_type
                .unwrap_or_else(|| DEFAULT_CONTENT_TYPE.to_string()),
            update_time,
        }
    }

    /// Handle error from GCS operations
    fn handle_gcs_error(err: &GcsError) -> Error {
        let code = match &err {
            GcsError::Response(resp) => match resp.code {
                404 => Code::NotFound,
                401 | 403 => Code::PermissionDenied,
                408 | 429 => Code::ResourceExhausted,
                500..=599 => Code::Unavailable,
                _ => Code::Unknown,
            },
            _ => Code::Internal,
        };

        make_err!(code, "GCS operation failed: {}", err)
    }

    /// Reading data from reader and upload in a single operation
    async fn read_and_upload_all(
        &self,
        object_path: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        max_size: u64,
    ) -> Result<(), Error> {
        let initial_capacity = core::cmp::min(max_size as usize, 10 * 1024 * 1024);
        let mut data = Vec::with_capacity(initial_capacity);
        let max_size = max_size as usize;
        let mut total_size = 0usize;

        while total_size < max_size {
            let to_read = core::cmp::min(self.resumable_chunk_size, max_size - total_size);
            let chunk = reader.consume(Some(to_read)).await?;

            if chunk.is_empty() {
                break;
            }

            data.extend_from_slice(&chunk);
            total_size += chunk.len();
        }

        self.write_object(object_path, data).await
    }

    /// Implementing a resumable upload using the resumable upload API
    async fn try_resumable_upload(
        &self,
        object_path: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        max_size: u64,
    ) -> Result<(), Error> {
        self.with_connection(|| async {
            let request = UploadObjectRequest {
                bucket: object_path.bucket.clone(),
                ..Default::default()
            };

            let mut metadata = HashMap::<String, String>::new();
            metadata.insert("name".to_string(), object_path.path.clone());

            // Use Multipart upload type with metadata
            let upload_type = UploadType::Multipart(Box::new(Object {
                name: object_path.path.clone(),
                content_type: Some(DEFAULT_CONTENT_TYPE.to_string()),
                metadata: Some(metadata),
                ..Default::default()
            }));

            // Prepare resumable upload
            let uploader = self
                .client
                .prepare_resumable_upload(&request, &upload_type)
                .await
                .map_err(|e| Self::handle_gcs_error(&e))?;

            // Upload data in chunks
            let mut offset: u64 = 0;
            let max_size = max_size as usize;
            let mut total_uploaded = 0usize;

            while total_uploaded < max_size {
                let to_read = core::cmp::min(self.resumable_chunk_size, max_size - total_uploaded);
                let chunk = reader.consume(Some(to_read)).await?;

                if chunk.is_empty() {
                    break;
                }

                let chunk_size = chunk.len() as u64;
                total_uploaded += chunk.len();

                let is_final = total_uploaded >= max_size || chunk.len() < to_read;
                let total_size = if is_final {
                    Some(offset + chunk_size)
                } else {
                    Some(max_size as u64)
                };

                let chunk_def = ChunkSize::new(offset, offset + chunk_size - 1, total_size);

                // Upload chunk
                let status = uploader
                    .upload_multiple_chunk(chunk, &chunk_def)
                    .await
                    .map_err(|e| Self::handle_gcs_error(&e))?;

                // Update offset for next chunk
                offset += chunk_size;

                if let UploadStatus::Ok(_) = status {
                    break;
                }
            }

            // If nothing was uploaded, finalizing with empty content
            if offset == 0 {
                let chunk_def = ChunkSize::new(0, 0, Some(0));
                uploader
                    .upload_multiple_chunk(Vec::new(), &chunk_def)
                    .await
                    .map_err(|e| Self::handle_gcs_error(&e))?;
            }

            // Check if the object exists
            match self.read_object_metadata(object_path).await? {
                Some(_) => Ok(()),
                None => Err(make_err!(
                    Code::Internal,
                    "Upload completed but object not found"
                )),
            }
        })
        .await
    }
}

impl Debug for GcsClient {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GcsClient")
            .field("resumable_chunk_size", &self.resumable_chunk_size)
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
            let request = GetObjectRequest {
                bucket: object_path.bucket.clone(),
                object: object_path.path.clone(),
                ..Default::default()
            };

            match self.client.get_object(&request).await {
                Ok(obj) => Ok(Some(self.convert_to_gcs_object(obj))),
                Err(err) => {
                    if let GcsError::Response(resp) = &err {
                        if resp.code == 404 {
                            return Ok(None);
                        }
                    }
                    Err(Self::handle_gcs_error(&err))
                }
            }
        })
        .await
    }

    async fn read_object_content(
        &self,
        object_path: &ObjectPath,
        start: u64,
        end: Option<u64>,
    ) -> Result<Box<dyn Stream<Item = Result<Bytes, Error>> + Send + Unpin>, Error> {
        type StreamItem = Result<Bytes, gcloud_storage::http::Error>;
        struct ReadStream<T: Stream<Item = StreamItem> + Send + Unpin> {
            stream: T,
            permit: Option<OwnedSemaphorePermit>,
        }

        impl<T: Stream<Item = StreamItem> + Send + Unpin> Stream for ReadStream<T> {
            type Item = Result<Bytes, Error>;

            fn poll_next(
                mut self: core::pin::Pin<&mut Self>,
                cx: &mut core::task::Context<'_>,
            ) -> core::task::Poll<Option<Self::Item>> {
                match std::pin::pin!(&mut self.stream).poll_next(cx) {
                    core::task::Poll::Ready(Some(Ok(bytes))) => {
                        core::task::Poll::Ready(Some(Ok(bytes)))
                    }
                    core::task::Poll::Ready(Some(Err(err))) => {
                        self.permit.take();
                        core::task::Poll::Ready(Some(Err(GcsClient::handle_gcs_error(&err))))
                    }
                    core::task::Poll::Ready(None) => {
                        self.permit.take();
                        core::task::Poll::Ready(None)
                    }
                    core::task::Poll::Pending => core::task::Poll::Pending,
                }
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                self.stream.size_hint()
            }
        }

        let permit =
            self.semaphore.clone().acquire_owned().await.map_err(|e| {
                make_err!(Code::Internal, "Failed to acquire connection permit: {}", e)
            })?;
        let request = GetObjectRequest {
            bucket: object_path.bucket.clone(),
            object: object_path.path.clone(),
            ..Default::default()
        };

        let start = (start > 0).then_some(start);
        let range = Range(start, end);

        // Download the object
        let stream = self
            .client
            .download_streamed_object(&request, &range)
            .await
            .map_err(|e| Self::handle_gcs_error(&e))?;

        Ok(Box::new(ReadStream {
            stream,
            permit: Some(permit),
        }))
    }

    async fn write_object(&self, object_path: &ObjectPath, content: Vec<u8>) -> Result<(), Error> {
        self.with_connection(|| async {
            let request = UploadObjectRequest {
                bucket: object_path.bucket.clone(),
                ..Default::default()
            };

            let media = Media::new(object_path.path.clone());
            let upload_type = UploadType::Simple(media);

            self.client
                .upload_object(&request, content, &upload_type)
                .await
                .map_err(|e| Self::handle_gcs_error(&e))?;

            Ok(())
        })
        .await
    }

    async fn start_resumable_write(&self, object_path: &ObjectPath) -> Result<String, Error> {
        self.with_connection(|| async {
            let request = UploadObjectRequest {
                bucket: object_path.bucket.clone(),
                ..Default::default()
            };

            let upload_type = UploadType::Multipart(Box::new(Object {
                name: object_path.path.clone(),
                content_type: Some(DEFAULT_CONTENT_TYPE.to_string()),
                ..Default::default()
            }));

            // Start resumable upload session
            let uploader = self
                .client
                .prepare_resumable_upload(&request, &upload_type)
                .await
                .map_err(|e| Self::handle_gcs_error(&e))?;

            Ok(uploader.url().to_string())
        })
        .await
    }

    async fn upload_chunk(
        &self,
        upload_url: &str,
        _object_path: &ObjectPath,
        data: Bytes,
        offset: u64,
        end_offset: u64,
        total_size: Option<u64>,
    ) -> Result<(), Error> {
        self.with_connection(|| async {
            let uploader = self.client.get_resumable_upload(upload_url.to_string());

            let last_byte = if end_offset == 0 { 0 } else { end_offset - 1 };
            let chunk_def = ChunkSize::new(offset, last_byte, total_size);

            // Upload chunk
            uploader
                .upload_multiple_chunk(data, &chunk_def)
                .await
                .map_err(|e| Self::handle_gcs_error(&e))?;

            Ok(())
        })
        .await
    }

    async fn upload_from_reader(
        &self,
        object_path: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        _upload_id: &str,
        max_size: u64,
    ) -> Result<(), Error> {
        let mut retry_count = 0;
        let mut retry_delay = INITIAL_UPLOAD_RETRY_DELAY_MS;

        loop {
            let result = if max_size < SIMPLE_UPLOAD_THRESHOLD {
                self.read_and_upload_all(object_path, reader, max_size)
                    .await
            } else {
                self.try_resumable_upload(object_path, reader, max_size)
                    .await
            };

            match result {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let is_retriable = matches!(
                        e.code,
                        Code::Unavailable | Code::ResourceExhausted | Code::DeadlineExceeded
                    ) || (e.code == Code::Internal
                        && e.to_string().contains("connection"));

                    if !is_retriable || retry_count >= MAX_UPLOAD_RETRIES {
                        return Err(e);
                    }

                    if let Err(reset_err) = reader.try_reset_stream() {
                        return Err(e.merge(reset_err));
                    }

                    sleep(Duration::from_millis(retry_delay)).await;
                    retry_delay = core::cmp::min(retry_delay * 2, MAX_UPLOAD_RETRY_DELAY_MS);

                    let mut rng = rand::rng();
                    let jitter_factor = rng.random::<f64>().mul_add(0.4, 0.8);
                    retry_delay = (retry_delay as f64 * jitter_factor) as u64;

                    retry_count += 1;
                }
            }
        }
    }

    async fn object_exists(&self, object_path: &ObjectPath) -> Result<bool, Error> {
        let metadata = self.read_object_metadata(object_path).await?;
        Ok(metadata.is_some())
    }
}
