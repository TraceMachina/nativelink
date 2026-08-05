// Copyright 2024 Trace Machina, Inc. All rights reserved.
//
// Licensed under the Business Source License, Version 1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may requested a copy of the License by emailing contact@nativelink.com.
//
// Use of this module requires an enterprise license agreement, which can be
// attained by emailing contact@nativelink.com or signing up for Nativelink
// Cloud at app.nativelink.com.
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! `LiveWorker`: one long-lived persistent-worker child process.
//!
//! A `LiveWorker` wraps a child process started with the action's tool +
//! startup-flag prefix + `--persistent_worker`. It owns the child's stdin/stdout
//! and exposes `dispatch(request) -> response` for the pool to invoke.
//!
//! Lifecycle:
//! - `spawn` → ready, never-used
//! - `dispatch` flips an in-flight bool, writes the request, reads the response,
//!   updates `last_used` and `request_count`
//! - `drop` sends SIGTERM, waits a grace period, then SIGKILL
//!
//! v1 is single-request-per-worker (no multiplex). The pool serializes
//! concurrent acquires of the same worker via its data structure.

use core::time::Duration;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

use bytes::BytesMut;
use nativelink_error::{Code, Error, ResultExt, make_err};
use prost::Message as ProstMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tracing::{debug, warn};

use super::protocol::{WireFormat, WorkRequest, WorkResponse};

/// Default time we wait for a single `WorkResponse` after writing a request.
/// Beyond this we kill the worker and surface a `DeadlineExceeded` error.
/// Callers may override via `LiveWorker::dispatch_with_timeout`.
const DEFAULT_DISPATCH_TIMEOUT: Duration = Duration::from_mins(10);

/// One persistent-worker child process.
#[derive(Debug)]
pub struct LiveWorker {
    /// Spawned child process. Kept on the struct so its Drop kills the process
    /// when the worker is dropped without `shutdown` being called.
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Wire format we negotiated at spawn (immutable for this worker's lifetime).
    wire_format: WireFormat,
    /// Number of successful `dispatch` calls completed.
    request_count: u64,
    /// When the last `dispatch` finished. Used by the pool's idle eviction.
    last_used: Instant,
    /// Working directory the child runs in. Stored so the pool can reuse it
    /// across requests for the same `WorkerKey` until per-request sandboxing
    /// arrives in v2.
    working_dir: PathBuf,
}

impl LiveWorker {
    /// Spawn a new persistent worker.
    ///
    /// `executable` is the resolved tool path (absolute, or relative to
    /// `working_dir`). `startup_args` are the flags that the worker is launched
    /// with once — these become part of the `WorkerKey`. The Bazel-conventional
    /// `--persistent_worker` flag is appended automatically.
    pub async fn spawn(
        executable: &Path,
        startup_args: &[String],
        wire_format: WireFormat,
        working_dir: &Path,
    ) -> Result<Self, Error> {
        let mut cmd = Command::new(executable);
        cmd.args(startup_args)
            .arg("--persistent_worker")
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // WorkResponse.output carries per-action diagnostics. The child
            // stderr stream is process-lifetime data and cannot be attributed
            // safely to a single request.
            .stderr(Stdio::null())
            .kill_on_drop(true);

        debug!(
            ?executable,
            ?startup_args,
            ?wire_format,
            "Spawning persistent worker"
        );

        let mut child = cmd.spawn().err_tip(|| {
            format!(
                "Spawning persistent worker {} with args {startup_args:?}",
                executable.display()
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| make_err!(Code::Internal, "Persistent worker child has no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| make_err!(Code::Internal, "Persistent worker child has no stdout"))?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            wire_format,
            request_count: 0,
            last_used: Instant::now(),
            working_dir: working_dir.to_path_buf(),
        })
    }

    pub const fn wire_format(&self) -> WireFormat {
        self.wire_format
    }

    pub const fn request_count(&self) -> u64 {
        self.request_count
    }

    pub const fn last_used(&self) -> Instant {
        self.last_used
    }

    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    /// Returns true if the child process has already exited.
    pub fn is_dead(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)) | Err(_))
    }

    /// Send a single `WorkRequest` and read the matching `WorkResponse`. On
    /// any I/O error or response framing error, the worker is considered
    /// dead — the caller (pool) must not return it to the idle set.
    pub async fn dispatch(&mut self, request: &WorkRequest) -> Result<WorkResponse, Error> {
        self.dispatch_with_timeout(request, DEFAULT_DISPATCH_TIMEOUT)
            .await
    }

    pub async fn dispatch_with_timeout(
        &mut self,
        request: &WorkRequest,
        timeout: Duration,
    ) -> Result<WorkResponse, Error> {
        if request.request_id != 0 {
            return Err(make_err!(
                Code::InvalidArgument,
                "v1 persistent workers do not support multiplex; request_id must be 0, got {}",
                request.request_id
            ));
        }

        let bytes = request.encode_framed(self.wire_format)?;
        self.stdin
            .write_all(&bytes)
            .await
            .err_tip(|| "Writing WorkRequest to persistent worker stdin")?;
        self.stdin
            .flush()
            .await
            .err_tip(|| "Flushing persistent worker stdin")?;

        let read_fut = read_response(&mut self.stdout, self.wire_format);
        let response = if let Ok(result) = tokio::time::timeout(timeout, read_fut).await {
            result?
        } else {
            warn!(
                ?timeout,
                "Persistent worker did not respond before deadline; killing child"
            );
            drop(self.child.kill().await);
            return Err(make_err!(
                Code::DeadlineExceeded,
                "Persistent worker did not respond within {timeout:?}"
            ));
        };

        if response.request_id != 0 {
            // v1 contract: workers MUST echo request_id 0. If a tool ever
            // produces a multiplex-style id we don't know what to do with the
            // response — fail conservatively rather than misroute output.
            return Err(make_err!(
                Code::Internal,
                "Persistent worker returned non-zero request_id={}; v1 does not support multiplex",
                response.request_id
            ));
        }

        self.request_count += 1;
        self.last_used = Instant::now();
        Ok(response)
    }

    /// Gracefully drain: close stdin so the worker observes EOF and exits on
    /// its own, then wait up to `grace`, then SIGKILL if still alive. Always
    /// consumes the worker.
    pub async fn shutdown(mut self, grace: Duration) {
        // Dropping stdin closes the pipe, which most well-behaved workers
        // interpret as "no more work, please exit".
        drop(self.stdin);

        match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(Ok(status)) => debug!(?status, "Persistent worker exited cleanly"),
            Ok(Err(err)) => warn!(?err, "Error waiting for persistent worker exit"),
            Err(_) => {
                warn!(
                    ?grace,
                    "Persistent worker did not exit within grace; SIGKILLing"
                );
                drop(self.child.kill().await);
            }
        }
    }
}

/// Read one framed `WorkResponse` from the worker's stdout.
async fn read_response(
    stdout: &mut BufReader<ChildStdout>,
    format: WireFormat,
) -> Result<WorkResponse, Error> {
    match format {
        WireFormat::Proto => {
            // Length-delimited varint frame. Decode the varint manually because
            // we want to read exactly the right number of bytes without buffering
            // arbitrary data from the worker's pipe.
            let len = read_varint(stdout).await?;
            if len > 64 * 1024 * 1024 {
                return Err(make_err!(
                    Code::OutOfRange,
                    "Persistent worker WorkResponse length {len} exceeds 64 MiB cap"
                ));
            }
            let mut buf = BytesMut::with_capacity(len);
            buf.resize(len, 0);
            stdout
                .read_exact(&mut buf)
                .await
                .err_tip(|| "Reading WorkResponse proto body from persistent worker")?;
            <WorkResponse as ProstMessage>::decode(&buf[..])
                .map_err(|e| make_err!(Code::Internal, "Decoding WorkResponse proto body: {e}"))
        }
        WireFormat::Json => {
            // Newline-delimited JSON.
            let mut line = Vec::with_capacity(256);
            loop {
                let mut byte = [0u8; 1];
                let n = stdout
                    .read(&mut byte)
                    .await
                    .err_tip(|| "Reading WorkResponse JSON byte from persistent worker")?;
                if n == 0 {
                    return Err(make_err!(
                        Code::Aborted,
                        "Persistent worker closed stdout before sending a full JSON response"
                    ));
                }
                if byte[0] == b'\n' {
                    if line.is_empty() {
                        continue; // tolerate blank lines between responses
                    }
                    break;
                }
                line.push(byte[0]);
                if line.len() > 64 * 1024 * 1024 {
                    return Err(make_err!(
                        Code::OutOfRange,
                        "Persistent worker WorkResponse JSON line exceeds 64 MiB cap"
                    ));
                }
            }
            WorkResponse::decode_framed(&line, WireFormat::Json)
        }
    }
}

/// Read a protobuf varint from the reader. Reused for the length prefix of a
/// length-delimited frame.
async fn read_varint(r: &mut BufReader<ChildStdout>) -> Result<usize, Error> {
    let mut result: u64 = 0;
    for shift in (0..64).step_by(7) {
        let mut byte = [0u8; 1];
        let n = r
            .read(&mut byte)
            .await
            .err_tip(|| "Reading varint byte from persistent worker stdout")?;
        if n == 0 {
            return Err(make_err!(
                Code::Aborted,
                "Persistent worker closed stdout while reading varint"
            ));
        }
        result |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return usize::try_from(result).map_err(|_| {
                make_err!(
                    Code::OutOfRange,
                    "Varint length {result} does not fit in usize"
                )
            });
        }
    }
    Err(make_err!(
        Code::OutOfRange,
        "Varint did not terminate within 10 bytes"
    ))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use nativelink_macro::nativelink_test;

    use super::*;

    struct TestWorkerProgram {
        executable: PathBuf,
        startup_args: Vec<String>,
    }

    impl TestWorkerProgram {
        fn startup_args(&self) -> &[String] {
            &self.startup_args
        }
    }

    #[cfg(unix)]
    fn echo_script(working_dir: &Path, unix_body: &str, _windows_body: &str) -> TestWorkerProgram {
        let path = working_dir.join("worker.sh");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(unix_body.as_bytes()).unwrap();
        file.sync_all().unwrap();
        drop(file);

        TestWorkerProgram {
            executable: PathBuf::from("/bin/sh"),
            startup_args: vec![path.display().to_string()],
        }
    }

    #[cfg(windows)]
    fn echo_script(working_dir: &Path, _unix_body: &str, windows_body: &str) -> TestWorkerProgram {
        let path = working_dir.join("worker.ps1");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(windows_body.as_bytes()).unwrap();
        file.sync_all().unwrap();
        drop(file);

        TestWorkerProgram {
            executable: PathBuf::from("powershell.exe"),
            startup_args: vec![
                "-NoProfile".to_owned(),
                "-ExecutionPolicy".to_owned(),
                "Bypass".to_owned(),
                "-File".to_owned(),
                path.display().to_string(),
            ],
        }
    }

    #[nativelink_test]
    async fn shutdown_kills_unresponsive_worker() {
        // A worker that never reads/writes — shutdown grace expires, we SIGKILL.
        let dir = tempfile::tempdir().unwrap();
        let script = echo_script(dir.path(), "exec sleep 30\n", "Start-Sleep -Seconds 30\n");
        let worker = LiveWorker::spawn(
            &script.executable,
            script.startup_args(),
            WireFormat::Json,
            dir.path(),
        )
        .await
        .unwrap();
        let start = Instant::now();
        worker.shutdown(Duration::from_millis(100)).await;
        // SIGKILL should arrive well within a second.
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[nativelink_test]
    async fn dispatch_json_round_trip() {
        // A worker scripted to read one JSON line and echo a canned response.
        let dir = tempfile::tempdir().unwrap();
        let script = echo_script(
            dir.path(),
            // Read one line, ignore it, emit a fixed response. Newline-terminated.
            "read line\necho '{\"exitCode\":0,\"output\":\"ok\"}'\n",
            "$line = [Console]::In.ReadLine()\n[Console]::Out.WriteLine('{\"exitCode\":0,\"output\":\"ok\"}')\n",
        );
        let mut worker = LiveWorker::spawn(
            &script.executable,
            script.startup_args(),
            WireFormat::Json,
            dir.path(),
        )
        .await
        .unwrap();

        let req = WorkRequest {
            arguments: vec!["compile".into()],
            ..WorkRequest::default()
        };
        let resp = worker.dispatch(&req).await.unwrap();
        assert_eq!(resp.exit_code, 0);
        assert_eq!(resp.output, "ok");
        assert_eq!(worker.request_count(), 1);

        worker.shutdown(Duration::from_secs(1)).await;
    }

    #[nativelink_test]
    async fn dispatch_timeout_kills_worker() {
        let dir = tempfile::tempdir().unwrap();
        // Worker reads, then sleeps forever instead of responding.
        let script = echo_script(
            dir.path(),
            "read line\nexec sleep 60\n",
            "$line = [Console]::In.ReadLine()\nStart-Sleep -Seconds 60\n",
        );
        let mut worker = LiveWorker::spawn(
            &script.executable,
            script.startup_args(),
            WireFormat::Json,
            dir.path(),
        )
        .await
        .unwrap();

        let req = WorkRequest {
            arguments: vec!["compile".into()],
            ..WorkRequest::default()
        };
        let result = worker
            .dispatch_with_timeout(&req, Duration::from_millis(100))
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, Code::DeadlineExceeded);
        assert!(worker.is_dead());
    }

    #[nativelink_test]
    async fn rejects_multiplex_request_id() {
        let dir = tempfile::tempdir().unwrap();
        let script = echo_script(
            dir.path(),
            "read line\necho '{\"exitCode\":0}'\n",
            "$line = [Console]::In.ReadLine()\n[Console]::Out.WriteLine('{\"exitCode\":0}')\n",
        );
        let mut worker = LiveWorker::spawn(
            &script.executable,
            script.startup_args(),
            WireFormat::Json,
            dir.path(),
        )
        .await
        .unwrap();
        let req = WorkRequest {
            request_id: 7,
            ..WorkRequest::default()
        };
        let err = worker.dispatch(&req).await.unwrap_err();
        assert_eq!(err.code, Code::InvalidArgument);
        worker.shutdown(Duration::from_secs(1)).await;
    }
}
