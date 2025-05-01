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

use core::fmt::{Debug, Formatter};
use core::future::Future;
use core::time::Duration;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use google_cloud_auth::credentials::CredentialsFile;
use google_cloud_storage::client::{Client, ClientConfig};
use google_cloud_storage::http::Error as GcsError;
use google_cloud_storage::http::objects::Object;
use google_cloud_storage::http::objects::download::Range;
use google_cloud_storage::http::objects::get::GetObjectRequest;
use google_cloud_storage::http::objects::upload::{Media, UploadObjectRequest, UploadType};
use google_cloud_storage::http::resumable_upload_client::{ChunkSize, UploadStatus};
use nativelink_config::stores::ExperimentalGcsSpec;
use nativelink_error::{Code, Error, make_err};
use nativelink_util::buf_channel::DropCloserReadHalf;
use rand::Rng;
use tokio::fs;
use tokio::sync::Semaphore;
use tokio::time::sleep;

use crate::gcs_client::types::{
    CHUNK_SIZE, DEFAULT_CONCURRENT_UPLOADS, DEFAULT_CONTENT_TYPE, GcsObject,
    INITIAL_UPLOAD_RETRY_DELAY_MS, MAX_UPLOAD_RETRIES, MAX_UPLOAD_RETRY_DELAY_MS, ObjectPath,
    SIMPLE_UPLOAD_THRESHOLD, Timestamp,
};

/// A trait that defines the required GCS operations.
/// This abstraction allows for easier testing by mocking GCS responses.
#[async_trait]
pub trait GcsOperations: Send + Sync + Debug {
    /// Read metadata for a GCS object
    async fn read_object_metadata(&self, object: &ObjectPath) -> Result<Option<GcsObject>, Error>;

    /// Read the content of a GCS object, optionally with a range
    async fn read_object_content(
        &self,
        object_path: &ObjectPath,
        start: i64,
        end: Option<i64>,
    ) -> Result<Vec<u8>, Error>;

    /// Write object with simple upload (for smaller objects)
    async fn write_object(&self, object_path: &ObjectPath, content: Vec<u8>) -> Result<(), Error>;

    /// Start a resumable write operation and return the upload URL
    async fn start_resumable_write(&self, object_path: &ObjectPath) -> Result<String, Error>;

    /// Upload a chunk of data in a resumable upload session
    async fn upload_chunk(
        &self,
        upload_url: &str,
        object_path: &ObjectPath,
        data: Vec<u8>,
        offset: i64,
        end_offset: i64,
        is_final: bool,
    ) -> Result<(), Error>;

    /// Complete high-level operation to upload data from a reader
    async fn upload_from_reader(
        &self,
        object_path: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        upload_id: &str,
        max_size: i64,
    ) -> Result<(), Error>;

    /// Check if an object exists
    async fn object_exists(&self, object_path: &ObjectPath) -> Result<bool, Error>;
}

/// Main client for interacting with Google Cloud Storage
pub struct GcsClient {
    client: Client,
    resumable_chunk_size: usize,
    semaphore: Arc<Semaphore>,
}

impl GcsClient {
    /// Create a new GCS client from the provided spec
    pub async fn new(spec: &ExperimentalGcsSpec) -> Result<Self, Error> {
        // Creating default config without authentication initially
        let mut client_config = ClientConfig::default();
        let mut auth_success = false;

        // Trying authentication with credentials file path from environment variable.
        // Google Cloud Auth expects the path to the credentials file to be specified
        // in the GOOGLE_APPLICATION_CREDENTIALS environment variable.
        // Check https://cloud.google.com/docs/authentication/application-default-credentials#GAC
        // for more details.
        if let Ok(creds_path) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
            let path_str = PathBuf::from(creds_path).to_string_lossy().to_string();

            tracing::info!("Attempting to load credentials from: {}", path_str);
            match CredentialsFile::new_from_file(path_str.clone()).await {
                Ok(creds_file) => {
                    match ClientConfig::default().with_credentials(creds_file).await {
                        Ok(config) => {
                            tracing::info!("Successfully authenticated with credentials file");
                            client_config = config;
                            auth_success = true;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to configure client with credentials: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to load credentials from file: {}", e);
                }
            }
        }

        // Trying JSON credentials in environment variable if first method failed
        if !auth_success {
            if let Ok(creds_json) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS_JSON") {
                tracing::info!("Attempting to use credentials from environment variable");

                // Creating temporary file
                let temp_dir = std::env::temp_dir();
                let temp_path = temp_dir.join(format!("gcs-creds-{}.json", uuid::Uuid::new_v4()));
                let temp_path_str = temp_path.to_string_lossy().to_string();

                // Writing JSON to temporary file
                if let Err(e) = fs::write(&temp_path, creds_json).await {
                    tracing::warn!("Failed to write credentials to temp file: {}", e);
                } else {
                    // Load credentials from temporary file
                    match CredentialsFile::new_from_file(temp_path_str).await {
                        Ok(creds_file) => {
                            match ClientConfig::default().with_credentials(creds_file).await {
                                Ok(config) => {
                                    tracing::info!(
                                        "Successfully authenticated with JSON credentials"
                                    );
                                    client_config = config;
                                    auth_success = true;
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Failed to configure client with JSON credentials: {}",
                                        e
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to load credentials from JSON: {}", e);
                        }
                    }

                    // Clean up temporary file
                    drop(fs::remove_file(temp_path).await);
                }
            }
        }

        if !auth_success {
            match ClientConfig::default().with_auth().await {
                Ok(config) => {
                    tracing::info!(
                        "Successfully authenticated with application default credentials"
                    );
                    client_config = config;
                }
                Err(e) => {
                    tracing::warn!("Failed to use application default credentials: {}", e);
                    tracing::info!("Continuing with unauthenticated access");
                }
            }
        }

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
    fn handle_gcs_error(&self, err: &GcsError) -> Error {
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
        max_size: i64,
    ) -> Result<(), Error> {
        let initial_capacity = core::cmp::min(max_size as usize, 10 * 1024 * 1024);
        let mut data = Vec::with_capacity(initial_capacity);
        let mut total_size: i64 = 0;

        while total_size < max_size {
            let to_read =
                core::cmp::min(self.resumable_chunk_size, (max_size - total_size) as usize);
            let chunk = reader.consume(Some(to_read)).await?;

            if chunk.is_empty() {
                break;
            }

            data.extend_from_slice(&chunk);
            total_size += chunk.len() as i64;
        }

        self.write_object(object_path, data).await
    }

    /// Implementing a resumable upload using the resumable upload API
    async fn try_resumable_upload(
        &self,
        object_path: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        max_size: i64,
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
                .map_err(|e| self.handle_gcs_error(&e))?;

            // Upload data in chunks
            let mut offset: u64 = 0;
            let mut total_uploaded: i64 = 0;

            while total_uploaded < max_size {
                let to_read = core::cmp::min(
                    self.resumable_chunk_size,
                    (max_size - total_uploaded) as usize,
                );
                let chunk = reader.consume(Some(to_read)).await?;

                if chunk.is_empty() {
                    break;
                }

                let chunk_size = chunk.len() as u64;
                total_uploaded += chunk.len() as i64;

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
                    .map_err(|e| self.handle_gcs_error(&e))?;

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
                    .map_err(|e| self.handle_gcs_error(&e))?;
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

#[async_trait]
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
                    Err(self.handle_gcs_error(&err))
                }
            }
        })
        .await
    }

    async fn read_object_content(
        &self,
        object_path: &ObjectPath,
        start: i64,
        end: Option<i64>,
    ) -> Result<Vec<u8>, Error> {
        self.with_connection(|| async {
            let request = GetObjectRequest {
                bucket: object_path.bucket.clone(),
                object: object_path.path.clone(),
                ..Default::default()
            };

            let range = if start > 0 || end.is_some() {
                Range(
                    if start > 0 { Some(start as u64) } else { None },
                    end.map(|e| e as u64),
                )
            } else {
                Range(None, None)
            };

            // Download the object
            let content = self
                .client
                .download_object(&request, &range)
                .await
                .map_err(|e| self.handle_gcs_error(&e))?;

            Ok(content)
        })
        .await
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
                .map_err(|e| self.handle_gcs_error(&e))?;

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
                .map_err(|e| self.handle_gcs_error(&e))?;

            Ok(uploader.url().to_string())
        })
        .await
    }

    async fn upload_chunk(
        &self,
        upload_url: &str,
        _object_path: &ObjectPath,
        data: Vec<u8>,
        offset: i64,
        end_offset: i64,
        is_final: bool,
    ) -> Result<(), Error> {
        self.with_connection(|| async {
            let uploader = self.client.get_resumable_upload(upload_url.to_string());

            let total_size = if is_final {
                Some(end_offset as u64)
            } else {
                None
            };

            let chunk_def = ChunkSize::new(offset as u64, end_offset as u64 - 1, total_size);

            // Upload chunk
            uploader
                .upload_multiple_chunk(data, &chunk_def)
                .await
                .map_err(|e| self.handle_gcs_error(&e))?;

            Ok(())
        })
        .await
    }

    async fn upload_from_reader(
        &self,
        object_path: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        _upload_id: &str,
        max_size: i64,
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
                    let jitter_factor = 0.8 + (rng.random::<f64>() * 0.4);
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
