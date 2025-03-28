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
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use hyper::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use hyper::Method;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use nativelink_config::stores::GcsSpec;
use nativelink_error::{make_err, Code, Error};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

use crate::gcs_client::types::{
    build_request, create_https_client, execute_request, Client, ObjectPath, AUDIENCE,
    ENV_AUTH_TOKEN, ENV_PRIVATE_KEY, ENV_SERVICE_EMAIL, MAX_DELAY, MAX_REFRESH_ATTEMPTS,
    REFRESH_WINDOW, RETRY_DELAY_BASE, SCOPE, TOKEN_ENDPOINT, TOKEN_LIFETIME,
};

#[derive(Debug, Serialize)]
struct JwtClaims {
    iss: String,
    sub: String,
    aud: String,
    iat: u64,
    exp: u64,
    scope: String,
}

#[derive(Clone, Debug)]
struct TokenInfo {
    token: String,
    refresh_at: u64,
}

#[derive(Deserialize, Debug)]
struct TokenResponse {
    access_token: String,
    #[allow(dead_code)]
    expires_in: u64,
    #[allow(dead_code)]
    token_type: String,
}

#[async_trait]
pub trait AuthProvider: Send + Sync + Debug {
    async fn get_token(&self) -> Result<String, Error>;

    async fn create_auth_headers(&self, _object: &ObjectPath) -> Result<HeaderMap, Error> {
        let mut headers = HeaderMap::new();

        let token = self.get_token().await?;

        headers.insert(
            hyper::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| make_err!(Code::Internal, "Invalid token"))?,
        );

        headers.insert(
            hyper::header::ACCEPT,
            HeaderValue::from_static("application/json"),
        );

        Ok(headers)
    }
}

#[derive(Debug)]
pub struct GcsAuthProvider {
    token_cache: RwLock<Option<TokenInfo>>,
    refresh_lock: Mutex<()>,
    service_email: String,
    private_key: String,
    http_client: Client,

    // Store configuration values
    scope: String,
    audience: String,
    token_lifetime: std::time::Duration,
    refresh_window: std::time::Duration,
    max_delay: std::time::Duration,
    max_refresh_attempts: u32,
    retry_delay_base: std::time::Duration,
}

impl GcsAuthProvider {
    fn prepare_key_format(key: &str) -> String {
        let key = key.replace("\\n", "\n");
        let begin_marker = "-----BEGIN ";
        let end_marker = "-----END ";
        let key_type = "PRIVATE KEY";

        if !key.contains(&format!("{}{}{}", begin_marker, key_type, "-----")) {
            format!(
                "{}{}{}\n{}\n{}{}{}",
                begin_marker,
                key_type,
                "-----",
                key.trim(),
                end_marker,
                key_type,
                "-----"
            )
        } else {
            key
        }
    }

    pub async fn from_env() -> Result<Self, Error> {
        // First trying environment variables for direct token
        if let Ok(token) = std::env::var(ENV_AUTH_TOKEN) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| make_err!(Code::Internal, "System time error: {}", e))?
                .as_secs();

            let http_client = create_https_client();

            return Ok(Self {
                token_cache: RwLock::new(Some(TokenInfo {
                    token,
                    refresh_at: now + TOKEN_LIFETIME.as_secs() - REFRESH_WINDOW.as_secs(),
                })),
                refresh_lock: Mutex::new(()),
                service_email: String::new(),
                private_key: String::new(),
                http_client,
                scope: SCOPE.to_string(),
                audience: AUDIENCE.to_string(),
                token_lifetime: TOKEN_LIFETIME,
                refresh_window: REFRESH_WINDOW,
                max_delay: MAX_DELAY,
                max_refresh_attempts: MAX_REFRESH_ATTEMPTS,
                retry_delay_base: RETRY_DELAY_BASE,
            });
        }

        // Primary Way: Using the service account credentials from the env variable
        let service_email = std::env::var(ENV_SERVICE_EMAIL).map_err(|_| {
            make_err!(
                Code::InvalidArgument,
                "GCS_SERVICE_EMAIL environment variable not set"
            )
        })?;

        let private_key = std::env::var(ENV_PRIVATE_KEY).map_err(|_| {
            make_err!(
                Code::InvalidArgument,
                "GCS_PRIVATE_KEY environment variable not set"
            )
        })?;

        let private_key = Self::prepare_key_format(&private_key);
        let http_client = create_https_client();

        Ok(Self {
            token_cache: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            service_email,
            private_key,
            http_client,
            scope: SCOPE.to_string(),
            audience: AUDIENCE.to_string(),
            token_lifetime: TOKEN_LIFETIME,
            refresh_window: REFRESH_WINDOW,
            max_delay: MAX_DELAY,
            max_refresh_attempts: MAX_REFRESH_ATTEMPTS,
            retry_delay_base: RETRY_DELAY_BASE,
        })
    }

    pub async fn new(spec: &GcsSpec) -> Result<Self, Error> {
        let service_email = if spec.service_email.is_empty() {
            // Fallback to environment variable
            std::env::var(ENV_SERVICE_EMAIL).map_err(|_| {
                make_err!(
                    Code::InvalidArgument,
                    "No service email provided in spec or environment"
                )
            })?
        } else {
            spec.service_email.clone()
        };

        // Get and format private key from environment
        let private_key = if let Ok(key) = std::env::var(ENV_PRIVATE_KEY) {
            Self::prepare_key_format(&key)
        } else {
            // If an auth token is provided in environment, allow empty private key
            if std::env::var(ENV_AUTH_TOKEN).is_ok() {
                String::new()
            } else {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "No private key provided in environment variable GCS_PRIVATE_KEY"
                ));
            }
        };

        let http_client = create_https_client();

        // If no private key but auth token exists, we'll use that
        if private_key.is_empty() {
            if let Ok(token) = std::env::var(ENV_AUTH_TOKEN) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| make_err!(Code::Internal, "System time error: {}", e))?
                    .as_secs();

                let token_lifetime = spec
                    .token_lifetime_secs
                    .map_or(TOKEN_LIFETIME, std::time::Duration::from_secs);
                let refresh_window = spec
                    .token_refresh_window_secs
                    .map_or(REFRESH_WINDOW, std::time::Duration::from_secs);

                return Ok(Self {
                    token_cache: RwLock::new(Some(TokenInfo {
                        token,
                        refresh_at: now + token_lifetime.as_secs() - refresh_window.as_secs(),
                    })),
                    refresh_lock: Mutex::new(()),
                    service_email,
                    private_key: String::new(),
                    http_client,
                    scope: spec.auth_scope.clone().unwrap_or_else(|| SCOPE.to_string()),
                    audience: spec
                        .auth_audience
                        .clone()
                        .unwrap_or_else(|| AUDIENCE.to_string()),
                    token_lifetime,
                    refresh_window,
                    max_delay: spec
                        .max_token_refresh_delay_secs
                        .map_or(MAX_DELAY, std::time::Duration::from_secs),
                    max_refresh_attempts: spec
                        .max_token_refresh_attempts
                        .unwrap_or(MAX_REFRESH_ATTEMPTS),
                    retry_delay_base: spec
                        .token_retry_delay_base_secs
                        .map_or(RETRY_DELAY_BASE, std::time::Duration::from_secs),
                });
            }
        }

        // Normal initialization with service account email and private key
        Ok(Self {
            token_cache: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            service_email,
            private_key,
            http_client,
            scope: spec.auth_scope.clone().unwrap_or_else(|| SCOPE.to_string()),
            audience: spec
                .auth_audience
                .clone()
                .unwrap_or_else(|| AUDIENCE.to_string()),
            token_lifetime: spec
                .token_lifetime_secs
                .map_or(TOKEN_LIFETIME, std::time::Duration::from_secs),
            refresh_window: spec
                .token_refresh_window_secs
                .map_or(REFRESH_WINDOW, std::time::Duration::from_secs),
            max_delay: spec
                .max_token_refresh_delay_secs
                .map_or(MAX_DELAY, std::time::Duration::from_secs),
            max_refresh_attempts: spec
                .max_token_refresh_attempts
                .unwrap_or(MAX_REFRESH_ATTEMPTS),
            retry_delay_base: spec
                .token_retry_delay_base_secs
                .map_or(RETRY_DELAY_BASE, std::time::Duration::from_secs),
        })
    }

    fn calculate_retry_delay(&self, attempt: u32) -> std::time::Duration {
        let base = self.retry_delay_base.as_secs() as u32;
        let delay = 2_u32.saturating_pow(attempt.saturating_sub(1)) * base;
        std::cmp::min(
            std::time::Duration::from_secs(u64::from(delay)),
            self.max_delay,
        )
    }

    fn add_jitter(duration: std::time::Duration) -> std::time::Duration {
        let jitter_factor = rand::rng().random_range(0.8..=1.2);
        let millis = (duration.as_millis() as f64 * jitter_factor) as u64;
        std::time::Duration::from_millis(millis)
    }

    async fn generate_token(&self) -> Result<TokenInfo, Error> {
        // If no private key is available, try to get token from environment
        if self.private_key.is_empty() {
            let token = std::env::var(ENV_AUTH_TOKEN).map_err(|_| {
                make_err!(
                    Code::Unauthenticated,
                    "No private key provided and no auth token in environment"
                )
            })?;

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| make_err!(Code::Internal, "System time error: {}", e))?
                .as_secs();

            return Ok(TokenInfo {
                token,
                refresh_at: now + self.token_lifetime.as_secs() - self.refresh_window.as_secs(),
            });
        }

        // Create JWT token
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| make_err!(Code::Internal, "System time error: {}", e))?
            .as_secs();

        let expiry = now + self.token_lifetime.as_secs();
        let refresh_at = expiry - self.refresh_window.as_secs();

        let claims = JwtClaims {
            iss: self.service_email.clone(),
            sub: self.service_email.clone(),
            aud: self.audience.clone(),
            iat: now,
            exp: expiry,
            scope: self.scope.clone(),
        };

        let header = Header::new(Algorithm::RS256);

        let key = EncodingKey::from_rsa_pem(self.private_key.as_bytes())
            .map_err(|e| make_err!(Code::InvalidArgument, "Invalid private key: {}", e))?;

        let jwt = encode(&header, &claims, &key)
            .map_err(|e| make_err!(Code::Internal, "JWT encoding failed: {}", e))?;

        // Exchange JWT for OAuth token
        let payload = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={jwt}"
        );

        let mut request_headers = HeaderMap::new();
        request_headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );

        let uri = TOKEN_ENDPOINT
            .parse::<hyper::Uri>()
            .map_err(|e| make_err!(Code::InvalidArgument, "Invalid URI: {}", e))?;

        let request = build_request(Method::POST, uri, request_headers, Some(payload)).await?;

        let (status, body_bytes) = execute_request(&self.http_client, request).await?;

        if !status.is_success() {
            let error_text = String::from_utf8_lossy(&body_bytes);
            return Err(make_err!(
                Code::Unauthenticated,
                "Failed to get access token: HTTP {} - {}",
                status,
                error_text
            ));
        }

        let token_response: TokenResponse = serde_json::from_slice(&body_bytes)
            .map_err(|e| make_err!(Code::Internal, "Failed to parse token response: {}", e))?;

        Ok(TokenInfo {
            token: token_response.access_token,
            refresh_at,
        })
    }

    async fn refresh_token_with_retry(&self) -> Result<TokenInfo, Error> {
        let mut attempt = 1;
        let mut last_error = None;

        while attempt <= self.max_refresh_attempts {
            match self.generate_token().await {
                Ok(token_info) => return Ok(token_info),
                Err(e) => {
                    eprintln!(
                        "Token refresh attempt {}/{} failed: {}",
                        attempt, self.max_refresh_attempts, e
                    );
                    last_error = Some(e);

                    if attempt < self.max_refresh_attempts {
                        let delay = Self::add_jitter(self.calculate_retry_delay(attempt));
                        tokio::time::sleep(delay).await;
                    }

                    attempt += 1;
                }
            }
        }

        Err(make_err!(
            Code::Internal,
            "Token refresh failed after {} attempts: {}",
            self.max_refresh_attempts,
            last_error.unwrap()
        ))
    }
}

#[async_trait]
impl AuthProvider for GcsAuthProvider {
    async fn get_token(&self) -> Result<String, Error> {
        // Helper function to check if token is valid and return it if it is
        let check_token_validity = |token_info: &TokenInfo| -> Result<Option<String>, Error> {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| make_err!(Code::Internal, "System time error: {}", e))?
                .as_secs();

            if now < token_info.refresh_at {
                Ok(Some(token_info.token.clone()))
            } else {
                Ok(None)
            }
        };

        // Fast path: check if we have a valid token
        if let Some(token_info) = self.token_cache.read().await.as_ref() {
            if let Some(token) = check_token_validity(token_info)? {
                return Ok(token);
            }
        }

        // Slow path: need to refresh token
        let _guard = self.refresh_lock.lock().await;

        if let Some(token_info) = self.token_cache.read().await.as_ref() {
            if let Some(token) = check_token_validity(token_info)? {
                return Ok(token);
            }
        }

        // Generating and caching the new token
        let token_info = self.refresh_token_with_retry().await?;
        *self.token_cache.write().await = Some(token_info.clone());

        Ok(token_info.token)
    }
}

#[derive(Debug)]
pub struct NoAuthProvider;

#[async_trait]
impl AuthProvider for NoAuthProvider {
    async fn get_token(&self) -> Result<String, Error> {
        Ok(String::from("no-auth-token"))
    }
}
