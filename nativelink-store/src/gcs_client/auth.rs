use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use nativelink_config::stores::GcsSpec;
use nativelink_error::{make_err, Code, Error};
use rand::Rng;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

// If you update these values, please update the GcsSpec as well.
const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const AUDIENCE: &str = "https://storage.googleapis.com/";
const TOKEN_LIFETIME: Duration = Duration::from_secs(3600); // 1 hour
const REFRESH_WINDOW: Duration = Duration::from_secs(300); // 5 minutes
const MAX_DELAY: Duration = Duration::from_secs(30);
const MAX_REFRESH_ATTEMPTS: u32 = 5;
const RETRY_DELAY_BASE: Duration = Duration::from_secs(2);

const ENV_PRIVATE_KEY: &str = "GCS_PRIVATE_KEY";
const ENV_AUTH_TOKEN: &str = "GOOGLE_AUTH_TOKEN";

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

pub struct GcsAuth {
    token_cache: RwLock<Option<TokenInfo>>,
    refresh_lock: Mutex<()>,
    service_email: String,
    private_key: String,
    // Store configuration values
    scope: String,
    audience: String,
    token_lifetime: Duration,
    refresh_window: Duration,
    max_delay: Duration,
    max_refresh_attempts: u32,
    retry_delay_base: Duration,
}

impl GcsAuth {
    fn format_private_key(key: &str) -> String {
        // Replace literal '\n' with actual newlines and ensure proper PEM format
        let key = key.replace("\\n", "\n");
        if !key.contains("-----BEGIN PRIVATE KEY-----") {
            format!(
                "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
                key.trim()
            )
        } else {
            key
        }
    }

    pub async fn new(spec: &GcsSpec) -> Result<Self, Error> {
        // First try to get direct token from environment
        if let Ok(token) = std::env::var(ENV_AUTH_TOKEN) {
            let token_lifetime = spec
                .token_lifetime_secs
                .map_or(TOKEN_LIFETIME, Duration::from_secs);
            let refresh_window = spec
                .token_refresh_window_secs
                .map_or(REFRESH_WINDOW, Duration::from_secs);

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| make_err!(Code::Internal, "System time error: {}", e))?
                .as_secs();

            return Ok(Self {
                token_cache: RwLock::new(Some(TokenInfo {
                    token,
                    refresh_at: now + token_lifetime.as_secs() - refresh_window.as_secs(),
                })),
                refresh_lock: Mutex::new(()),
                service_email: String::new(),
                private_key: String::new(),
                scope: spec.auth_scope.clone().unwrap_or_else(|| SCOPE.to_string()),
                audience: spec
                    .auth_audience
                    .clone()
                    .unwrap_or_else(|| AUDIENCE.to_string()),
                token_lifetime,
                refresh_window,
                max_delay: spec
                    .max_token_refresh_delay_secs
                    .map_or(MAX_DELAY, Duration::from_secs),
                max_refresh_attempts: spec
                    .max_token_refresh_attempts
                    .unwrap_or(MAX_REFRESH_ATTEMPTS),
                retry_delay_base: spec
                    .token_retry_delay_base_secs
                    .map_or(RETRY_DELAY_BASE, Duration::from_secs),
            });
        }

        // Get and format private key
        let private_key = std::env::var(ENV_PRIVATE_KEY).map_err(|_| {
            make_err!(
                Code::NotFound,
                "Environment variable {} not found",
                ENV_PRIVATE_KEY
            )
        })?;

        let private_key = Self::format_private_key(&private_key);

        Ok(Self {
            token_cache: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            service_email: spec.service_email.clone(),
            private_key,
            scope: spec.auth_scope.clone().unwrap_or_else(|| SCOPE.to_string()),
            audience: spec
                .auth_audience
                .clone()
                .unwrap_or_else(|| AUDIENCE.to_string()),
            token_lifetime: spec
                .token_lifetime_secs
                .map_or(TOKEN_LIFETIME, Duration::from_secs),
            refresh_window: spec
                .token_refresh_window_secs
                .map_or(REFRESH_WINDOW, Duration::from_secs),
            max_delay: spec
                .max_token_refresh_delay_secs
                .map_or(MAX_DELAY, Duration::from_secs),
            max_refresh_attempts: spec
                .max_token_refresh_attempts
                .unwrap_or(MAX_REFRESH_ATTEMPTS),
            retry_delay_base: spec
                .token_retry_delay_base_secs
                .map_or(RETRY_DELAY_BASE, Duration::from_secs),
        })
    }

    fn calculate_retry_delay(&self, attempt: u32) -> Duration {
        let delay = self.retry_delay_base * (2_u32.pow(attempt.saturating_sub(1)));
        std::cmp::min(delay, self.max_delay)
    }

    fn add_jitter(duration: Duration) -> Duration {
        let mut rng = rand::thread_rng();
        let jitter_factor = rng.gen_range(0.8..1.2);
        duration.mul_f64(jitter_factor)
    }

    async fn generate_token(&self) -> Result<TokenInfo, Error> {
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

        let token = encode(&header, &claims, &key)
            .map_err(|e| make_err!(Code::Internal, "JWT encoding failed: {}", e))?;

        Ok(TokenInfo { token, refresh_at })
    }

    async fn refresh_token_with_retry(&self) -> Result<TokenInfo, Error> {
        let mut attempt = 1;
        let mut last_error = None;

        while attempt <= self.max_refresh_attempts {
            match self.generate_token().await {
                Ok(token_info) => return Ok(token_info),
                Err(e) => {
                    println!(
                        "Token refresh attempt {attempt}/{} failed: {e}",
                        self.max_refresh_attempts
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

    pub async fn get_valid_token(&self) -> Result<String, Error> {
        // Fast path: check if we have a valid token
        if let Some(token_info) = self.token_cache.read().await.as_ref() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| make_err!(Code::Internal, "System time error: {}", e))?
                .as_secs();

            if now < token_info.refresh_at {
                return Ok(token_info.token.clone());
            }
        }

        // Slow path: need to refresh token
        let _guard = self.refresh_lock.lock().await;

        // Double-check after acquiring lock
        let current_info = self.token_cache.read().await.clone();
        if let Some(token_info) = &current_info {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| make_err!(Code::Internal, "System time error: {}", e))?
                .as_secs();

            if now < token_info.refresh_at {
                return Ok(token_info.token.clone());
            }
        }

        // Either refresh existing token or create new one
        let token_info = if self.private_key.is_empty() {
            // Fallback to environment token
            if let Ok(token) = std::env::var(ENV_AUTH_TOKEN) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| make_err!(Code::Internal, "System time error: {}", e))?
                    .as_secs();

                TokenInfo {
                    token,
                    refresh_at: now + self.token_lifetime.as_secs() - self.refresh_window.as_secs(),
                }
            } else {
                return Err(make_err!(
                    Code::Unauthenticated,
                    "No valid authentication method available"
                ));
            }
        } else {
            self.refresh_token_with_retry().await?
        };

        *self.token_cache.write().await = Some(token_info.clone());
        Ok(token_info.token)
    }
}
