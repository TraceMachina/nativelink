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

use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use hyper::client::connect::Connect;
use hyper::{header, Method, StatusCode, Uri};
use nativelink_config::stores::GcsSpec;
use nativelink_error::{make_err, Code, Error};
use nativelink_util::buf_channel::DropCloserReadHalf;
use rand::Rng;

use crate::gcs_client::auth::{AuthProvider, GcsAuthProvider};
use crate::gcs_client::connector::GcsTlsConnector;
use crate::gcs_client::operations::GcsOperations;
use crate::gcs_client::types::{
    GcsObject, Object, ObjectPath, Timestamp, API_BASE_URL, CHUNK_SIZE, DEFAULT_CONTENT_TYPE,
    INITIAL_UPLOAD_RETRY_DELAY_MS, MAX_UPLOAD_RETRIES, MAX_UPLOAD_RETRY_DELAY_MS,
    SIMPLE_UPLOAD_THRESHOLD, UPLOAD_BASE_URL,
};

pub struct GcsClient<C = GcsTlsConnector>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    client: hyper::Client<C>,
    auth_provider: Arc<dyn AuthProvider>,
    endpoint: Option<String>, // Optional endpoint override
    resumable_chunk_size: usize,
}

impl GcsClient {
    pub async fn new(spec: &GcsSpec) -> Result<Self, Error> {
        let jitter_amt = spec.retry.jitter;
        let jitter_fn = Arc::new(move |delay: tokio::time::Duration| {
            if jitter_amt == 0.0 {
                return delay;
            }
            let min = 1.0 - (jitter_amt / 2.0);
            let max = 1.0 + (jitter_amt / 2.0);
            let mut rng = rand::rng();
            let factor = min + (max - min) * rng.random::<f32>();
            delay.mul_f32(factor)
        });
        let connector = GcsTlsConnector::new(spec, jitter_fn.clone());
        let pool_timeout = spec.pool_idle_timeout_secs.unwrap_or(30);
        let client = hyper::Client::builder()
            .pool_idle_timeout(Duration::from_secs(pool_timeout))
            .build(connector);

        let auth_provider = Arc::new(GcsAuthProvider::new(spec).await?);
        let resumable_chunk_size = spec.resumable_chunk_size.unwrap_or(CHUNK_SIZE);

        Ok(Self {
            client,
            auth_provider,
            endpoint: spec.endpoint.clone(),
            resumable_chunk_size,
        })
    }
}

impl<C> GcsClient<C>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    pub async fn new_with_http_client(
        spec: &GcsSpec,
        client: hyper::Client<C>,
    ) -> Result<Self, Error> {
        let auth_provider = Arc::new(GcsAuthProvider::new(spec).await?);
        let resumable_chunk_size = spec.resumable_chunk_size.unwrap_or(CHUNK_SIZE);

        Ok(Self {
            client,
            auth_provider,
            endpoint: spec.endpoint.clone(),
            resumable_chunk_size,
        })
    }

    // For testing: construct with custom auth provider
    #[cfg(test)]
    pub fn with_auth_provider(
        client: hyper::Client<C>,
        auth_provider: Arc<dyn AuthProvider>,
        endpoint: Option<String>,
    ) -> Self {
        Self {
            client,
            auth_provider,
            endpoint,
            resumable_chunk_size: CHUNK_SIZE,
        }
    }

    fn get_api_base_url(&self) -> &str {
        if let Some(endpoint) = &self.endpoint {
            endpoint
        } else {
            API_BASE_URL
        }
    }

    fn get_upload_base_url(&self) -> &str {
        if let Some(endpoint) = &self.endpoint {
            endpoint
        } else {
            UPLOAD_BASE_URL
        }
    }

    async fn parse_object_metadata(&self, body_bytes: &[u8]) -> Result<GcsObject, Error> {
        let api_object: Object = serde_json::from_slice(body_bytes)
            .map_err(|e| make_err!(Code::Internal, "Failed to parse object metadata: {}", e))?;

        let size = match &api_object.size {
            Some(size_str) => size_str
                .parse::<i64>()
                .map_err(|_| make_err!(Code::Internal, "Failed to parse size as i64"))?,
            None => 0,
        };

        // Convert from API object to our GcsObject
        let update_time = if let Some(updated) = &api_object.updated {
            // Either we can use the chrono crate to parse the RFC3339 timestamp, or we'll
            // have to manually parse it. Using the chrono seems for now for reliability.
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(updated) {
                Some(Timestamp {
                    seconds: dt.timestamp(),
                    nanos: dt.timestamp_subsec_nanos() as i32,
                })
            } else {
                None
            }
        } else {
            None
        };

        Ok(GcsObject {
            name: api_object.name,
            bucket: api_object.bucket,
            size,
            content_type: api_object
                .content_type
                .unwrap_or_else(|| DEFAULT_CONTENT_TYPE.to_string()),
            update_time,
        })
    }

    async fn execute_request(
        &self,
        method: Method,
        uri: Uri,
        headers: header::HeaderMap,
        body: Option<Vec<u8>>,
    ) -> Result<(StatusCode, Bytes), Error> {
        let mut builder = hyper::Request::builder()
            .method(method)
            .uri(uri)
            .version(hyper::Version::HTTP_11);

        // Adding all the headers
        let headers_mut = builder
            .headers_mut()
            .ok_or_else(|| make_err!(Code::Internal, "Failed to get mutable headers"))?;

        for (k, v) in &headers {
            headers_mut.insert(k, v.clone());
        }

        // Building the request with the body
        let req = if let Some(body_data) = body {
            builder.body(hyper::Body::from(body_data))
        } else {
            builder.body(hyper::Body::empty())
        }
        .map_err(|e| make_err!(Code::InvalidArgument, "Failed to build request: {}", e))?;

        // Execute the request
        let response = self
            .client
            .request(req)
            .await
            .map_err(|e| make_err!(Code::Unavailable, "HTTP request failed: {}", e))?;

        let status = response.status();
        let body_bytes = hyper::body::to_bytes(response.into_body())
            .await
            .map_err(|e| make_err!(Code::Internal, "Failed to collect response body: {}", e))?;

        if !status.is_success() && status != StatusCode::NOT_FOUND {
            let body_str = String::from_utf8_lossy(&body_bytes);

            if status == StatusCode::NOT_FOUND {
                return Err(make_err!(
                    Code::NotFound,
                    "Resource not found: {}",
                    body_str
                ));
            }

            // Handle auth failures separately
            if status == StatusCode::UNAUTHORIZED {
                return Err(make_err!(
                    Code::Unauthenticated,
                    "Authentication failed: {}",
                    body_str
                ));
            }

            return Err(make_err!(
                match status.as_u16() {
                    401 | 403 => Code::PermissionDenied,
                    404 => Code::NotFound,
                    408 | 429 => Code::ResourceExhausted,
                    500..=599 => Code::Unavailable,
                    _ => Code::Unknown,
                },
                "HTTP error {}: {}",
                status,
                body_str
            ));
        }

        Ok((status, body_bytes))
    }

    async fn read_and_upload_all(
        &self,
        object: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        max_size: i64,
    ) -> Result<(), Error> {
        let initial_capacity = std::cmp::min(max_size as usize, 10 * 1024 * 1024); // Cap at 10MB initially
        let mut data = Vec::with_capacity(initial_capacity);
        let mut total_size: i64 = 0;

        while total_size < max_size {
            let to_read =
                std::cmp::min(self.resumable_chunk_size, (max_size - total_size) as usize);
            let chunk = reader.consume(Some(to_read)).await?;

            if chunk.is_empty() {
                break;
            }

            data.extend_from_slice(&chunk);
            total_size += chunk.len() as i64;
        }

        self.write_object(object, data).await
    }

    async fn try_resumable_upload(
        &self,
        object: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        max_size: i64,
    ) -> Result<(), Error> {
        let upload_url = self.start_resumable_write(object).await?;

        // Stream data in chunks
        let mut offset = 0i64;
        let mut has_more_data = true;
        let mut total_uploaded = 0i64;

        while has_more_data {
            let to_read = std::cmp::min(self.resumable_chunk_size, (max_size - offset) as usize);
            let chunk = reader.consume(Some(to_read)).await?;

            let chunk_size = chunk.len() as i64;
            total_uploaded += chunk_size;
            has_more_data = !chunk.is_empty() && total_uploaded < max_size;

            if !chunk.is_empty() {
                // Determining if this is the final chunk, and uploading it
                let is_final = !has_more_data;

                self.upload_chunk(
                    &upload_url,
                    object,
                    chunk.to_vec(),
                    offset,
                    offset + chunk_size,
                    is_final,
                )
                .await?;
                offset += chunk_size;
            } else {
                // No more data to read
                has_more_data = false;
                if offset == 0 {
                    self.upload_chunk(&upload_url, object, Vec::new(), 0, 0, true)
                        .await?;
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await; // Small delay to allow GCS to finalize

        match self.read_object_metadata(object.clone()).await? {
            Some(_) => Ok(()),
            None => Err(make_err!(
                Code::Internal,
                "Resumable upload failed - uploaded object not found"
            )),
        }
    }
}

impl<C> Debug for GcsClient<C>
where
    C: 'static + Clone + Connect + Send + Sync,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcsClient")
            .field("endpoint", &self.endpoint)
            .field("resumable_chunk_size", &self.resumable_chunk_size)
            .field("auth_provider", &self.auth_provider)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl<C> GcsOperations for GcsClient<C>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    async fn read_object_metadata(&self, object: ObjectPath) -> Result<Option<GcsObject>, Error> {
        let uri = format!(
            "{}/b/{}/o/{}",
            self.get_api_base_url(),
            urlencoding::encode(&object.bucket),
            urlencoding::encode(&object.path)
        )
        .parse::<Uri>()
        .map_err(|e| make_err!(Code::InvalidArgument, "Invalid URI: {}", e))?;

        let headers = self.auth_provider.create_auth_headers(&object).await?;
        let result = self.execute_request(Method::GET, uri, headers, None).await;

        match result {
            Ok((StatusCode::OK, body_bytes)) => {
                let gcs_object = self.parse_object_metadata(&body_bytes).await?;
                Ok(Some(gcs_object))
            }
            Err(e) if e.code == Code::NotFound => Ok(None),
            Err(e) => Err(e),
            _ => Ok(None),
        }
    }

    async fn read_object_content(
        &self,
        object: ObjectPath,
        start: i64,
        end: Option<i64>,
    ) -> Result<Vec<u8>, Error> {
        let uri = format!(
            "{}/b/{}/o/{}?alt=media",
            self.get_api_base_url(),
            urlencoding::encode(&object.bucket),
            urlencoding::encode(&object.path)
        )
        .parse::<Uri>()
        .map_err(|e| make_err!(Code::InvalidArgument, "Invalid URI: {}", e))?;

        let mut headers = self.auth_provider.create_auth_headers(&object).await?;

        if start > 0 || end.is_some() {
            let range = match end {
                Some(end_value) => format!("bytes={}-{}", start, end_value - 1),
                None => format!("bytes={start}-"),
            };

            headers.insert(
                header::RANGE,
                header::HeaderValue::from_str(&range)
                    .map_err(|_| make_err!(Code::InvalidArgument, "Invalid range header"))?,
            );
        }

        let result = self.execute_request(Method::GET, uri, headers, None).await;

        match result {
            Ok((status, body_bytes)) => {
                if status == StatusCode::OK || status == StatusCode::PARTIAL_CONTENT {
                    Ok(body_bytes.to_vec())
                } else {
                    let body_str = String::from_utf8_lossy(&body_bytes);
                    Err(make_err!(
                        Code::Internal,
                        "Unexpected status: {} - {}",
                        status,
                        body_str
                    ))
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn write_object(&self, object: &ObjectPath, content: Vec<u8>) -> Result<(), Error> {
        let uri = format!(
            "{}/b/{}/o?uploadType=media&name={}",
            self.get_upload_base_url(),
            urlencoding::encode(&object.bucket),
            urlencoding::encode(&object.path)
        )
        .parse::<Uri>()
        .map_err(|e| make_err!(Code::InvalidArgument, "Invalid URI: {}", e))?;

        let mut headers = self.auth_provider.create_auth_headers(object).await?;
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static(DEFAULT_CONTENT_TYPE),
        );
        headers.insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from_str(&content.len().to_string())
                .map_err(|_| make_err!(Code::InvalidArgument, "Invalid content length"))?,
        );

        let (_, _) = self
            .execute_request(Method::POST, uri, headers, Some(content))
            .await?;

        Ok(())
    }

    async fn start_resumable_write(&self, object: &ObjectPath) -> Result<String, Error> {
        let uri = format!(
            "{}/b/{}/o?uploadType=resumable&name={}",
            self.get_upload_base_url(),
            urlencoding::encode(&object.bucket),
            urlencoding::encode(&object.path)
        )
        .parse::<Uri>()
        .map_err(|e| make_err!(Code::InvalidArgument, "Invalid URI: {}", e))?;

        let mut headers = self.auth_provider.create_auth_headers(object).await?;

        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        headers.insert(
            "X-Upload-Content-Type",
            header::HeaderValue::from_static(DEFAULT_CONTENT_TYPE),
        );

        headers.insert(
            "X-Goog-Upload-Protocol",
            header::HeaderValue::from_static("resumable"),
        );

        headers.insert(
            "X-Goog-Upload-Command",
            header::HeaderValue::from_static("start"),
        );

        let metadata = serde_json::json!({
            "name": object.path,
            "contentType": DEFAULT_CONTENT_TYPE,
        });

        let metadata_bytes = serde_json::to_vec(&metadata)
            .map_err(|e| make_err!(Code::Internal, "Failed to serialize object metadata: {}", e))?;

        let mut builder = hyper::Request::builder()
            .method(Method::POST)
            .uri(uri)
            .version(hyper::Version::HTTP_11);

        let headers_mut = builder
            .headers_mut()
            .ok_or_else(|| make_err!(Code::Internal, "Failed to get mutable headers"))?;

        for (k, v) in &headers {
            headers_mut.insert(k, v.clone());
        }

        let request = builder
            .body(hyper::Body::from(metadata_bytes))
            .map_err(|e| make_err!(Code::InvalidArgument, "Failed to build request: {}", e))?;

        // Sending request
        let response = self
            .client
            .request(request)
            .await
            .map_err(|e| make_err!(Code::Unavailable, "HTTP request failed: {}", e))?;

        let status = response.status();
        let headers = response.headers().clone();

        // Read body for error reporting
        let body_bytes = hyper::body::to_bytes(response.into_body())
            .await
            .map_err(|e| make_err!(Code::Internal, "Failed to collect response body: {}", e))?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            return Err(make_err!(
                Code::Internal,
                "Failed to start resumable upload: Status {} - {}",
                status,
                body_str
            ));
        }

        if let Some(location) = headers.get("Location").or_else(|| headers.get("location")) {
            if let Ok(url) = location.to_str() {
                return Ok(url.to_string());
            }
        }

        for header_name in [
            "X-Goog-Upload-URL",
            "x-goog-upload-url",
            "upload-url",
            "Upload-URL",
        ] {
            if let Some(upload_url) = headers.get(header_name) {
                if let Ok(url) = upload_url.to_str() {
                    return Ok(url.to_string());
                }
            }
        }

        // As a last resort, we'll try to find URL in response body
        if status == StatusCode::OK {
            let body_str = String::from_utf8_lossy(&body_bytes);
            if !body_str.is_empty() {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body_str) {
                    for field in ["uploadUrl", "upload_url", "resumableUrl", "resumable_url"] {
                        if let Some(url) = json.get(field).and_then(|v| v.as_str()) {
                            return Ok(url.to_string());
                        }
                    }
                }
            }
        }

        // If all else fails, we'll return an error
        Err(make_err!(
            Code::Internal,
            "Failed to extract upload URL from response"
        ))
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
        let content_range = if data.is_empty() && is_final {
            "bytes */0".to_string()
        } else if is_final {
            format!("bytes {}-{}/{}", offset, end_offset - 1, end_offset)
        } else {
            format!("bytes {}-{}/*", offset, end_offset - 1)
        };

        let mut headers = self.auth_provider.create_auth_headers(object).await?;

        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static(DEFAULT_CONTENT_TYPE),
        );

        headers.insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from_str(&data.len().to_string())
                .map_err(|_| make_err!(Code::InvalidArgument, "Invalid content length"))?,
        );

        headers.insert(
            header::CONTENT_RANGE,
            header::HeaderValue::from_str(&content_range).map_err(|_| {
                make_err!(
                    Code::InvalidArgument,
                    "Invalid content range: {}",
                    content_range
                )
            })?,
        );

        headers.insert(
            "X-Goog-Upload-Protocol",
            header::HeaderValue::from_static("resumable"),
        );

        let command = if is_final {
            "upload, finalize"
        } else {
            "upload"
        };

        headers.insert(
            "X-Goog-Upload-Command",
            header::HeaderValue::from_str(command)
                .map_err(|_| make_err!(Code::InvalidArgument, "Invalid upload command"))?,
        );

        headers.insert(
            "X-Goog-Upload-Offset",
            header::HeaderValue::from_str(&offset.to_string())
                .map_err(|_| make_err!(Code::InvalidArgument, "Invalid offset"))?,
        );

        let uri = upload_url
            .parse::<Uri>()
            .map_err(|e| make_err!(Code::InvalidArgument, "Invalid URI: {}", e))?;

        let (status, _) = self
            .execute_request(Method::PUT, uri, headers, Some(data))
            .await?;

        match status.as_u16() {
            200 | 201 | 308 => Ok(()),
            _ => Err(make_err!(
                Code::Internal,
                "Unexpected status during resumable upload: {}",
                status
            )),
        }
    }

    async fn upload_from_reader(
        &self,
        object: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        _upload_id: &str,
        max_size: i64,
    ) -> Result<(), Error> {
        let mut retry_count = 0;
        let mut retry_delay = INITIAL_UPLOAD_RETRY_DELAY_MS;

        loop {
            let result = if max_size < SIMPLE_UPLOAD_THRESHOLD {
                self.read_and_upload_all(object, reader, max_size).await
            } else {
                self.try_resumable_upload(object, reader, max_size).await
            };

            match result {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let is_retriable = match e.code {
                        Code::Unavailable | Code::ResourceExhausted | Code::DeadlineExceeded => {
                            true
                        }
                        Code::Internal if e.to_string().contains("connection") => true,
                        _ => false,
                    };

                    if !is_retriable || retry_count >= MAX_UPLOAD_RETRIES {
                        return Err(e);
                    }

                    if let Err(reset_err) = reader.try_reset_stream() {
                        return Err(e.merge(reset_err));
                    }

                    tokio::time::sleep(Duration::from_millis(retry_delay)).await;
                    retry_delay = std::cmp::min(retry_delay * 2, MAX_UPLOAD_RETRY_DELAY_MS);

                    let mut rng = rand::rng();
                    let jitter_factor = 0.8 + (rng.random::<f64>() * 0.4);
                    retry_delay = (retry_delay as f64 * jitter_factor) as u64;

                    retry_count += 1;
                }
            }
        }
    }

    async fn object_exists(&self, object: &ObjectPath) -> Result<bool, Error> {
        let metadata = self.read_object_metadata(object.clone()).await?;
        Ok(metadata.is_some())
    }
}
