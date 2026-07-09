#![allow(dead_code)]

pub(crate) mod downloader;
pub(crate) mod extractor;
pub(crate) mod process;

use core::time::Duration;
use std::fs;
use std::path::PathBuf;
use std::sync::LazyLock;

pub(crate) use downloader::DownloadProgress;
use downloader::{download_file_with_callback, get_download_url, get_os};
use extractor::extract;
use mongodb::bson::{Bson, doc};
use nativelink_error::{Error, ResultExt, make_err};
use process::MongoProcess;
use tempfile::tempdir;
use tokio::sync::Mutex;
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
    /// Port to bind. 0 means pick a free ephemeral port at start time (and
    /// pick a fresh one if a bind race is lost). The port that was actually
    /// bound is reflected in the returned process's `connection_string`.
    pub port: u16,
    pub bind_ip: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// Serializes download+extract across concurrently running tests in this
/// process. The `extracted.marker` file is the completion predicate: re-check
/// it after acquiring the lock, because another test may have completed the
/// download while we waited. If a download fails the lock is simply released,
/// so the next waiter retries instead of hanging forever.
static DOWNLOAD_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// An ephemeral port is released back to the OS before mongod binds it, so
/// another process can grab it in between. When that happens our mongod exits
/// (or a foreign server answers on the port); `wait_until_ready` detects both,
/// and we retry with a fresh port.
const MAX_SPAWN_ATTEMPTS: u32 = 5;

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
            port: 0,
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
            let _download_guard = DOWNLOAD_LOCK.lock().await;
            // Re-check under the lock: another test may have just finished
            // downloading while we waited for the lock.
            if !extract_marker.exists() {
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
                fs::write(extract_marker, "")?;
                info!("Downloaded and extracted Mongo");
            }
        }

        let os = get_os()?;
        let auth_enabled = self.username.is_some() && self.password.is_some();

        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        let is_unix_socket = self.bind_ip.contains('/') || self.bind_ip.ends_with(".sock");

        let mut process = None;
        let mut last_error = make_err!(Code::Internal, "mongod was never spawned");
        for attempt in 0..MAX_SPAWN_ATTEMPTS {
            let port = if is_unix_socket || self.port != 0 {
                self.port
            } else {
                get_ephemeral_port()?
            };
            let uri = if is_unix_socket {
                // Minimal URL encoding for path, replacing / with %2F.
                // This is required for the rust mongodb driver to recognize it as a socket.
                // Note: When using sockets with mongodb crate, we often need to ensure the host is just the encoded path.
                let encoded = self.bind_ip.replace('/', "%2F");
                format!("mongodb://{encoded}/?directConnection=true")
            } else {
                format!("mongodb://{}:{port}/?directConnection=true", self.bind_ip)
            };

            let mut candidate = MongoProcess::start(
                &extract_target,
                port,
                &self.db_path,
                &os,
                &self.bind_ip,
                auth_enabled,
                uri,
            )
            .err_tip(|| format!("Starting mongo from {}", extract_target.display()))?;

            match wait_until_ready(&mut candidate, auth_enabled).await {
                Ok(()) => {
                    process = Some(candidate);
                    break;
                }
                // Dropping `candidate` kills the failed process.
                Err(err) => {
                    debug!(attempt, ?err, "mongod did not become ready");
                    last_error = err;
                }
            }
        }
        let Some(mut process) = process else {
            return Err(last_error);
        };

        if let (Some(username), Some(password)) = (&self.username, &self.password) {
            callback(InitStatus::SettingUpUser);
            let client_options = ready_client_options(&process.connection_string).await?;
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
                    callback(InitStatus::VerifyingCredentials);
                    // Try to authenticate
                    let mut auth_opts = client_options;
                    auth_opts.credential = Some(
                        mongodb::options::Credential::builder()
                            .username(username.clone())
                            .password(password.clone())
                            .source("admin".to_string())
                            .build(),
                    );

                    let auth_client = mongodb::Client::with_options(auth_opts)?;
                    // Verify by running a command that requires auth. Returning
                    // an error drops `process`, which kills the mongod.
                    if let Err(auth_err) = auth_client
                        .database("admin")
                        .run_command(doc! { "ping": 1 })
                        .await
                    {
                        return Err(make_err!(
                            Code::PermissionDenied,
                            "Authentication failed or invalid credentials provided: {}",
                            auth_err
                        ));
                    }
                }
            }

            // Update connection string to include credentials
            let final_uri = if is_unix_socket {
                let encoded = self.bind_ip.replace('/', "%2F");
                // For sockets, credentials go in the beginning
                format!("mongodb://{username}:{password}@{encoded}")
            } else {
                format!(
                    "mongodb://{username}:{password}@{}:{}/",
                    self.bind_ip, process.port
                )
            };
            process.connection_string = final_uri;
        }

        callback(InitStatus::DBInitialized);
        Ok(process)
    }
}

/// Ask the OS for a currently-free TCP port.
fn get_ephemeral_port() -> Result<u16, Error> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .err_tip(|| "Binding an ephemeral port for mongod")?;
    Ok(listener
        .local_addr()
        .err_tip(|| "Getting ephemeral port for mongod")?
        .port())
}

/// Client options with short per-attempt timeouts so the readiness poll reacts
/// quickly; the overall deadline is enforced by `wait_until_ready`'s loop.
async fn ready_client_options(uri: &str) -> Result<mongodb::options::ClientOptions, Error> {
    let mut client_options = mongodb::options::ClientOptions::parse(uri).await?;
    client_options.connect_timeout = Some(Duration::from_millis(500));
    client_options.server_selection_timeout = Some(Duration::from_millis(500));
    Ok(client_options)
}

/// Waits until the just-spawned mongod answers on its connection string. Fails
/// fast if the process exits (e.g. it lost a bind race for its port) and
/// verifies the server answering is actually the process we spawned rather
/// than a foreign server that won the port.
async fn wait_until_ready(process: &mut MongoProcess, auth_enabled: bool) -> Result<(), Error> {
    let uri = process.connection_string.clone();
    let client_options = ready_client_options(&uri).await?;

    let start = std::time::Instant::now();
    debug!("Waiting for MongoDB to start at {uri}");
    while start.elapsed() < Duration::from_secs(30) {
        // A successful connection after our child died would be to whatever
        // foreign process owns the port now, so check liveness first.
        if let Some(status) = process.try_wait()? {
            return Err(make_err!(
                Code::Internal,
                "mongod exited during startup with {status}"
            ));
        }

        let client = mongodb::Client::with_options(client_options.clone())?;
        match client.list_database_names().await {
            Ok(_) => {
                // With auth enabled we can't run serverStatus before
                // credentials exist, so skip the identity check there.
                if auth_enabled || is_our_mongod(&client, process).await? {
                    debug!("Connected to MongoDB at {uri}");
                    return Ok(());
                }
                return Err(make_err!(
                    Code::Internal,
                    "A different process is serving mongod's port"
                ));
            }
            Err(e) => {
                debug!("Connection attempt failed: {:?}", e);
                // If unauthorized error, it means we are connected but need auth, which is fine for readiness check
                // "Unauthorized" usually is error code 13
                if let mongodb::error::ErrorKind::Command(ref cmd_err) = *e.kind
                    && (cmd_err.code == 51 || cmd_err.code == 13 || cmd_err.code == 18)
                {
                    // 51: UserAlreadyExists?, 13: Unauthorized, 18: AuthFailed
                    return Ok(());
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Err(make_err!(
        Code::DeadlineExceeded,
        "Timed out waiting for MongoDB to start"
    ))
}

/// Compares `serverStatus.pid` against the pid of the child we spawned.
async fn is_our_mongod(client: &mongodb::Client, process: &MongoProcess) -> Result<bool, Error> {
    let status = client
        .database("admin")
        .run_command(doc! { "serverStatus": 1 })
        .await
        .map_err(|e| make_err!(Code::Internal, "serverStatus failed: {e}"))?;
    let server_pid = match status.get("pid") {
        Some(Bson::Int64(pid)) => *pid,
        Some(Bson::Int32(pid)) => i64::from(*pid),
        other => {
            return Err(make_err!(
                Code::Internal,
                "serverStatus returned unparsable pid: {other:?}"
            ));
        }
    };
    Ok(server_pid == i64::from(process.id()))
}
