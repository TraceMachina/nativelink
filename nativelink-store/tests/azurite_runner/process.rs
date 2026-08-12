use core::time::Duration;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;

use nativelink_error::{Error, ResultExt, make_err};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tonic::Code;
use tracing::info;
use uuid::Uuid;

#[derive(Debug)]
pub(crate) struct AzuriteProcess {
    child: Child,
    pub blob_endpoint: String,
    /// The port Azurite actually bound, as reported in its own startup
    /// banner, unlike `MongoProcess::port`, this isn't a port we asked
    /// for; we always request `0` (OS-assigned) since Azurite, unlike
    /// mongod, tells us what it settled on rather than us pre choosing it.
    pub port: u16,
    pub log_path: String,
    /// Populated by a background task for the process's whole lifetime, so
    /// a crash can be diagnosed after the fact, see `start`'s stdout
    /// comment for why nothing reads this eagerly/synchronously.
    stderr_lines: Arc<tokio::sync::Mutex<Vec<String>>>,
}

impl AzuriteProcess {
    pub(crate) async fn start(
        binary_path: &Path,
        location: &Path,
        bind_ip: &str,
    ) -> Result<Self, Error> {
        if !location.exists() {
            std::fs::create_dir_all(location)
                .err_tip(|| format!("Creating {}", location.display()))?;
        }

        let log_path = location
            .join(format!("azurite-{}.log", Uuid::new_v4().as_simple()))
            .to_string_lossy()
            .to_string();

        info!(?log_path, "Logging azurite-blob");

        let mut child = Command::new(binary_path)
            .arg("--blobPort")
            .arg("0")
            .arg("--blobHost")
            .arg(bind_ip)
            .arg("--location")
            .arg(location)
            .arg("--debug")
            .arg(&log_path)
            // Azurite's default supported API version can lag behind what
            // the `azure_storage_blob` SDK sends by default; without this,
            // otherwise valid requests are rejected outright.
            .arg("--skipApiVersionCheck")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .err_tip(|| format!("Spawning azurite-blob from {}", binary_path.display()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| make_err!(Code::Internal, "azurite-blob stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| make_err!(Code::Internal, "azurite-blob stderr was not piped"))?;

        // Drained continuously rather than read on demand, so that if the
        // process dies unexpectedly there's already a captured trail to
        // inspect via `stderr_snapshot`, reading it only after a failure
        // is detected would be too late, since the pipe's contents are
        // gone once the process exits.
        let stderr_lines = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr_lines_writer = stderr_lines.clone();
        nativelink_util::background_spawn!("azurite_stderr_drain", async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stderr_lines_writer.lock().await.push(line);
            }
        });

        // Drains stdout for the whole life of the process, not just until
        // the port's found. Azurite (via morgan) logs every request it
        // serves to stdout; if nothing keeps reading after the banner is
        // found, the pipe fills and Azurite's next write blocks/EPIPEs,
        // which crashes the entire Node process rather than just failing
        // one request, discovered when the very first real request after
        // startup reliably killed the server.
        let (port_tx, port_rx) = tokio::sync::oneshot::channel();
        nativelink_util::background_spawn!("azurite_stdout_drain", async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut port_tx = Some(port_tx);
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(tx) = port_tx.take() {
                    match parse_bound_port(&line) {
                        Some(port) => {
                            let _ = tx.send(port);
                        }
                        // Not the banner line yet (e.g. the "is starting
                        // on ..." line, which echoes our `0` input verbatim
                        // rather than the real bound port) keep waiting.
                        None => port_tx = Some(tx),
                    }
                }
            }
        });

        let port = tokio::time::timeout(Duration::from_secs(10), port_rx)
            .await
            .map_err(|_| {
                make_err!(
                    Code::DeadlineExceeded,
                    "Timed out waiting for azurite-blob's startup banner"
                )
            })?
            .map_err(|_| {
                make_err!(
                    Code::Internal,
                    "azurite-blob stdout closed before printing its bound port"
                )
            })?;

        Ok(Self {
            child,
            blob_endpoint: format!("http://{bind_ip}:{port}"),
            port,
            log_path,
            stderr_lines,
        })
    }

    pub(crate) fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Returns `Some(status)` if the child has exited.
    pub(crate) fn try_wait(&mut self) -> Result<Option<ExitStatus>, Error> {
        Ok(self.child.try_wait()?)
    }

    /// Returns whatever the background stderr-drain task has captured so
    /// far. Intended for diagnosing an unexpected exit, not for routine
    /// use, see the comment on `stderr_lines`.
    pub(crate) async fn stderr_snapshot(&self) -> Vec<String> {
        self.stderr_lines.lock().await.clone()
    }
}

impl Drop for AzuriteProcess {
    fn drop(&mut self) {
        // tokio::process::Child has no sync `wait`, and Drop can't be
        // async, so we fire-and-forget the kill signal rather than reap
        // the zombie, same reasoning as MongoProcess's Drop about never
        // panicking here, since this also runs during unwind from a failed
        // assertion.
        if let Ok(Some(_)) = self.child.try_wait() {
            return; // Already exited.
        }
        if let Err(e) = self.child.start_kill() {
            eprintln!("Failed to kill azurite-blob: {e}");
        }
    }
}

/// Parses the port out of Azurite's own banner line, e.g.
/// "Azurite Blob service successfully listens on <http://127.0.0.1:51833>"
fn parse_bound_port(line: &str) -> Option<u16> {
    let marker = "successfully listens on ";
    let idx = line.find(marker)?;
    let url_part = &line[idx + marker.len()..];
    url_part.rsplit(':').next()?.trim().parse().ok()
}
