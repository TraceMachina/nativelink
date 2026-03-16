#![allow(dead_code)]

pub(crate) mod downloader;
pub(crate) mod extractor;
pub(crate) mod process;

use core::time::Duration;
use std::fs;
use std::path::PathBuf;
use std::sync::{Condvar, LazyLock, Mutex};

pub(crate) use downloader::DownloadProgress;
use downloader::{download_file_with_callback, get_download_url, get_os};
use extractor::extract;
use mongodb::bson::doc;
use nativelink_error::{Error, ResultExt, make_err};
use process::MongoProcess;
use tempfile::tempdir;
use tonic::Code;
use tracing::{debug, info};

#[derive(Debug)]
pub(crate) enum InitStatus {
    CheckingDB,
    ValidatingInstallation,
    Downloading,
    DownloadProgress(DownloadProgress),
    SettingUpUser,
    VerifyingCredentials,
    DBInitialized,
}

#[derive(Debug)]
pub(crate) struct MongoEmbedded {
    pub version: String,
    pub download_path: PathBuf,
    pub extract_path: PathBuf,
    pub db_path: PathBuf,
    pub port: u16,
    pub bind_ip: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

static CURRENTLY_DOWNLOADING: LazyLock<(Mutex<bool>, Condvar)> =
    LazyLock::new(|| (Mutex::new(false), Condvar::new()));

impl MongoEmbedded {
    pub(crate) fn new(version: &str) -> Self {
        let cache_dir = dirs::cache_dir()
            .expect("Failed to find cache directory")
            .join("mongo");
        let db_path = tempdir()
            .expect("Failed to create temporary directory")
            .keep()
            .join("db");

        Self {
            version: version.to_string(),
            download_path: cache_dir.join("downloads"),
            extract_path: cache_dir.join("extracted"),
            db_path,
            port: 27017,
            bind_ip: "127.0.0.1".to_string(),
            username: None,
            password: None,
        }
    }

    pub(crate) const fn set_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub(crate) fn set_bind_ip(mut self, bind_ip: &str) -> Self {
        self.bind_ip = bind_ip.to_string();
        self
    }

    pub(crate) fn set_db_path(mut self, path: PathBuf) -> Self {
        self.db_path = path;
        self
    }

    pub(crate) fn set_credentials(mut self, username: &str, password: &str) -> Self {
        self.username = Some(username.to_string());
        self.password = Some(password.to_string());
        self
    }

    pub(crate) fn is_installed(&self) -> bool {
        let extract_target = self.extract_path.join(self.version.as_str());
        extract_target.exists()
    }

    pub(crate) async fn start(&self) -> Result<MongoProcess, Error> {
        self.start_with_progress(|_| {}).await
    }

    pub(crate) async fn start_with_progress<F>(
        &self,
        mut callback: F,
    ) -> Result<MongoProcess, Error>
    where
        F: FnMut(InitStatus),
    {
        callback(InitStatus::CheckingDB);
        let mongo_url = get_download_url(&self.version)?;
        let download_target = self.download_path.join(&mongo_url.filename);

        callback(InitStatus::ValidatingInstallation);

        let extract_target = self.extract_path.join(self.version.as_str());
        let extract_marker = extract_target.join("extracted.marker");

        if extract_marker.exists() {
            info!(?extract_marker, "Already have Mongo downloaded");
        } else {
            let existing_downloader = {
                let mut lock = CURRENTLY_DOWNLOADING.0.lock().unwrap();
                if *lock {
                    info!("Waiting for downloader");
                    let _guard = CURRENTLY_DOWNLOADING.1.wait(lock)?;
                    true
                } else {
                    info!("First here so, downloading Mongo");
                    *lock = true;
                    false
                }
            };
            if !existing_downloader {
                if !self.download_path.exists() {
                    fs::create_dir_all(&self.download_path)
                        .err_tip(|| format!("Making {}", self.download_path.display()))?;
                }
                callback(InitStatus::Downloading);
                if !download_target.exists() {
                    download_file_with_callback(&mongo_url.url, &download_target, |progress| {
                        callback(InitStatus::DownloadProgress(progress));
                    })
                    .await
                    .err_tip(|| {
                        format!(
                            "Downloading to {} from {}. If this breaks in coverage, edit tempHome",
                            download_target.display(),
                            mongo_url.url
                        )
                    })?;
                }
                extract(&download_target, &extract_target).err_tip(|| {
                    format!(
                        "Extraction from {} to {}",
                        download_target.display(),
                        extract_target.display()
                    )
                })?;
                info!("Downloaded, notifying");
                fs::write(extract_marker, "")?;
                let lock = CURRENTLY_DOWNLOADING.0.lock().unwrap();
                CURRENTLY_DOWNLOADING.1.notify_all();
                drop(lock);
            }
        }

        let os = get_os()?;

        // Calculate initial connection string for readiness check
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        let uri = if self.bind_ip.contains('/') || self.bind_ip.ends_with(".sock") {
            // Assume unix socket
            // Minimal URL encoding for path, replacing / with %2F.
            // This is required for the rust mongodb driver to recognize it as a socket.
            // Note: When using sockets with mongodb crate, we often need to ensure the host is just the encoded path.
            let encoded = self.bind_ip.replace('/', "%2F");
            format!("mongodb://{encoded}/?directConnection=true")
        } else {
            format!(
                "mongodb://{}:{}/?directConnection=true",
                self.bind_ip, self.port
            )
        };

        // Start process with auth flag if credentials are requested
        let auth_enabled = self.username.is_some() && self.password.is_some();
        let mut process = MongoProcess::start(
            &extract_target,
            self.port,
            &self.db_path,
            &os,
            &self.bind_ip,
            auth_enabled,
            uri.clone(),
        )
        .err_tip(|| format!("Starting mongo from {}", extract_target.display()))?;

        // Need to wait for it to be ready
        // We can try to connect
        let mut client_options = mongodb::options::ClientOptions::parse(&uri).await?;
        client_options.connect_timeout = Some(Duration::from_secs(2));
        client_options.server_selection_timeout = Some(Duration::from_secs(2));

        // Simple loop to wait for readiness
        let mut connected = false;
        let start = std::time::Instant::now();
        debug!("Waiting for MongoDB to start at {uri}");
        while start.elapsed() < Duration::from_secs(30) {
            let client = mongodb::Client::with_options(client_options.clone())?;
            match client.list_database_names().await {
                Ok(_) => {
                    connected = true;
                    debug!("Connected to MongoDB at {uri}");
                    break;
                }
                Err(e) => {
                    debug!("Connection attempt failed: {:?}", e);
                    // If unauthorized error, it means we are connected but need auth, which is fine for readiness check
                    // "Unauthorized" usually is error code 13
                    if let mongodb::error::ErrorKind::Command(ref cmd_err) = *e.kind {
                        if cmd_err.code == 51 || cmd_err.code == 13 || cmd_err.code == 18 {
                            // 51: UserAlreadyExists?, 13: Unauthorized, 18: AuthFailed
                            connected = true;
                            break;
                        }
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        if !connected {
            debug!("Timed out waiting for start.");
            process.kill()?;
            return Err(make_err!(
                Code::DeadlineExceeded,
                "Timed out waiting for MongoDB to start"
            ));
        }

        if let (Some(username), Some(password)) = (&self.username, &self.password) {
            callback(InitStatus::SettingUpUser);
            let client = mongodb::Client::with_options(client_options.clone())?;

            // Try to create user. This only works if localhost exception is active (no users)
            let db = client.database("admin");
            let run_cmd = db
                .run_command(doc! {
                   "createUser": username,
                   "pwd": password,
                   "roles": [
                       { "role": "root", "db": "admin" }
                   ]
                })
                .await;

            match run_cmd {
                Ok(_) => {
                    // Created user successfully
                }
                Err(_e) => {
                    // if needs_verify {
                    callback(InitStatus::VerifyingCredentials);
                    // Try to authenticate
                    let mut auth_opts = client_options.clone();
                    auth_opts.credential = Some(
                        mongodb::options::Credential::builder()
                            .username(username.clone())
                            .password(password.clone())
                            .source("admin".to_string())
                            .build(),
                    );

                    let auth_client = mongodb::Client::with_options(auth_opts)?;
                    // Verify by running a command that requires auth
                    if let Err(auth_err) = auth_client
                        .database("admin")
                        .run_command(doc! { "ping": 1 })
                        .await
                    {
                        process.kill()?;
                        return Err(make_err!(
                            Code::PermissionDenied,
                            "Authentication failed or invalid credentials provided: {}",
                            auth_err
                        ));
                    }
                }
            }

            // Update connection string to include credentials

            #[allow(clippy::case_sensitive_file_extension_comparisons)]
            let final_uri = if self.bind_ip.contains('/') || self.bind_ip.ends_with(".sock") {
                let encoded = self.bind_ip.replace('/', "%2F");
                // For sockets, credentials go in the beginning
                format!("mongodb://{username}:{password}@{encoded}")
            } else {
                format!(
                    "mongodb://{username}:{password}@{}:{}/",
                    self.bind_ip, self.port
                )
            };
            process.connection_string = final_uri;
        }

        callback(InitStatus::DBInitialized);
        Ok(process)
    }
}
