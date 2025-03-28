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

use std::time::Duration;

use bytes::Bytes;
use hyper::client::HttpConnector;
use hyper::{header, Body, Method, Request, StatusCode, Uri, Version};
use hyper_rustls::HttpsConnector;
use nativelink_error::{make_err, Code, Error};
use serde::{Deserialize, Serialize};

// NOTE: If you update these values, make sure to update the corresponding
// examples for the GcsSpec in nativelink-config/src/stores.rs.

// ----- File size thresholds -----
/// Threshold for using simple upload vs. resumable upload (10MB)
pub const SIMPLE_UPLOAD_THRESHOLD: i64 = 10 * 1024 * 1024;
/// Minimum size for multipart upload (5MB)
pub const MIN_MULTIPART_SIZE: u64 = 5 * 1024 * 1024;
/// Default chunk size for uploads (~2MB)
pub const CHUNK_SIZE: usize = 2 * 1024 * 1000;

// ----- Upload retry configuration -----
/// Maximum number of upload retry attempts
pub const MAX_UPLOAD_RETRIES: u32 = 5;
/// Initial delay between upload retry attempts (500ms)
pub const INITIAL_UPLOAD_RETRY_DELAY_MS: u64 = 500;
/// Maximum delay between upload retry attempts (8s)
pub const MAX_UPLOAD_RETRY_DELAY_MS: u64 = 8000;

// ----- Request configuration -----
/// Default content type for uploaded objects
pub const DEFAULT_CONTENT_TYPE: &str = "application/octet-stream";
/// Base URL for upload operations
pub const UPLOAD_BASE_URL: &str = "https://storage.googleapis.com/upload/storage/v1";
/// Base URL for API operations
pub const API_BASE_URL: &str = "https://storage.googleapis.com/storage/v1";

// ----- Connection pool configuration -----
/// Default number of concurrent uploads
pub const DEFAULT_CONCURRENT_UPLOADS: usize = 10;
/// Default buffer size for retrying requests (5MB)
pub const DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST: usize = 5 * 1024 * 1024;

// ----- Authentication configuration -----
/// OAuth scope for GCS operations
pub const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
/// Token audience
pub const AUDIENCE: &str = "https://www.googleapis.com/oauth2/v4/token";
/// Default token lifetime (1 hour)
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(3600);
/// Window before token expiry to refresh (5 minutes)
pub const REFRESH_WINDOW: Duration = Duration::from_secs(300);
/// Maximum delay between retry attempts (30 seconds)
pub const MAX_DELAY: Duration = Duration::from_secs(30);
/// Maximum number of token refresh attempts
pub const MAX_REFRESH_ATTEMPTS: u32 = 5;
/// Base delay for token refresh retry (2 seconds)
pub const RETRY_DELAY_BASE: Duration = Duration::from_secs(2);
/// Token endpoint URL
pub const TOKEN_ENDPOINT: &str = "https://www.googleapis.com/oauth2/v4/token";

// ----- Environment variable names -----
/// Environment variable for private key
pub const ENV_PRIVATE_KEY: &str = "GCS_PRIVATE_KEY";
/// Environment variable for service email
pub const ENV_SERVICE_EMAIL: &str = "GCS_SERVICE_EMAIL";
/// Environment variable for auth token
pub const ENV_AUTH_TOKEN: &str = "GOOGLE_AUTH_TOKEN";

pub type Client = hyper::Client<HttpsConnector<HttpConnector>>;

// Structs for GCS API
#[derive(Debug, Serialize, Deserialize)]
pub struct Object {
    pub name: String,
    pub bucket: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(rename = "timeCreated", skip_serializing_if = "Option::is_none")]
    pub time_created: Option<String>,
    #[serde(rename = "updated", skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    #[serde(rename = "metageneration", skip_serializing_if = "Option::is_none")]
    pub meta_generation: Option<String>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub additional_fields: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Default, Clone)]
pub struct GcsObject {
    pub name: String,
    pub bucket: String,
    pub size: i64,
    pub content_type: String,
    pub update_time: Option<Timestamp>,
}

#[derive(Debug, Clone)]
pub struct Timestamp {
    pub seconds: i64,
    pub nanos: i32,
}

#[derive(Clone, Debug)]
pub struct ObjectPath {
    pub bucket: String,
    pub(crate) path: String,
}

impl ObjectPath {
    pub fn new(bucket: String, path: &str) -> Self {
        let normalized_path = path.replace('\\', "/").trim_start_matches('/').to_string();
        Self {
            bucket,
            path: normalized_path,
        }
    }

    pub fn get_formatted_bucket(&self) -> String {
        format!("projects/_/buckets/{}", self.bucket)
    }
}

// HTTP Utilities
pub fn create_http_connector() -> HttpConnector {
    let mut http_connector = HttpConnector::new();
    http_connector.set_connect_timeout(Some(Duration::from_secs(10)));
    http_connector.enforce_http(false); // Allow HTTPS URLs
    http_connector.set_nodelay(true);
    http_connector
}

pub fn create_https_client() -> Client {
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .https_only()
        .enable_http1()
        .build();

    hyper::Client::builder()
        .pool_idle_timeout(Duration::from_secs(30))
        .build(https)
}

pub async fn build_request<T>(
    method: Method,
    uri: Uri,
    headers: header::HeaderMap,
    body: Option<T>,
) -> Result<Request<Body>, Error>
where
    T: Into<Body>,
{
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .version(Version::HTTP_11);

    let headers_mut = builder
        .headers_mut()
        .ok_or_else(|| make_err!(Code::Internal, "Failed to get mutable headers"))?;

    for (k, v) in &headers {
        headers_mut.insert(k, v.clone());
    }

    let req = if let Some(body_data) = body {
        builder.body(body_data.into())
    } else {
        builder.body(Body::empty())
    }
    .map_err(|e| make_err!(Code::InvalidArgument, "Failed to build request: {}", e))?;

    Ok(req)
}

pub async fn execute_request(
    client: &Client,
    request: Request<Body>,
) -> Result<(StatusCode, Bytes), Error> {
    let response = client
        .request(request)
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
