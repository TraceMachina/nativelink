// Copyright 2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License
// (the "License"); you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Integration test: 1 nativelink server + 3 workers exercising BlobsAvailable.
//!
//! Verifies the callback-based BlobsAvailable reporting pipeline:
//! 1. Workers connect and register with the scheduler
//! 2. Each worker sends an initial full-snapshot BlobsAvailable
//! 3. Blobs uploaded to a worker's CAS trigger the on_insert callback
//! 4. The next periodic tick sends a delta with just the new blobs
//! 5. The server processes notifications and populates the locality map
//! 6. When a worker disconnects, the server cleans up the locality map

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nativelink_proto::build::bazel::remote::execution::v2::{
    batch_update_blobs_request,
    content_addressable_storage_client::ContentAddressableStorageClient, BatchReadBlobsRequest,
    BatchUpdateBlobsRequest, Digest,
};
use sha2::{Digest as Sha2Digest, Sha256};
use tempfile::TempDir;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Allocate a free TCP port by binding to port 0 and extracting the OS-assigned port.
fn get_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

struct Ports {
    public: u16,
    worker_api: u16,
    cas: [u16; 3],
}

fn allocate_ports() -> Ports {
    Ports {
        public: get_free_port(),
        worker_api: get_free_port(),
        cas: [get_free_port(), get_free_port(), get_free_port()],
    }
}

/// Write a JSON5 config with 1 server (2 listeners) + 3 workers.
fn write_config(temp_dir: &Path, ports: &Ports) -> PathBuf {
    let d = temp_dir.to_string_lossy().replace('\\', "/");
    let config = format!(
        r#"{{
  stores: [
    {{ name: "AC_STORE", memory: {{ eviction_policy: {{ max_bytes: 100000000 }} }} }},
    {{ name: "SERVER_CAS", memory: {{ eviction_policy: {{ max_bytes: 100000000 }} }} }},
    {{
      name: "W1_STORE",
      fast_slow: {{
        fast: {{ filesystem: {{
          content_path: "{d}/w1/cas",
          temp_path: "{d}/w1/tmp",
          eviction_policy: {{ max_bytes: 100000000 }},
        }} }},
        slow: {{ grpc: {{
          instance_name: "main",
          endpoints: [{{ address: "grpc://127.0.0.1:{public}" }}],
          store_type: "cas",
        }} }},
        slow_direction: "get",
      }},
    }},
    {{
      name: "W2_STORE",
      fast_slow: {{
        fast: {{ filesystem: {{
          content_path: "{d}/w2/cas",
          temp_path: "{d}/w2/tmp",
          eviction_policy: {{ max_bytes: 100000000 }},
        }} }},
        slow: {{ grpc: {{
          instance_name: "main",
          endpoints: [{{ address: "grpc://127.0.0.1:{public}" }}],
          store_type: "cas",
        }} }},
        slow_direction: "get",
      }},
    }},
    {{
      name: "W3_STORE",
      fast_slow: {{
        fast: {{ filesystem: {{
          content_path: "{d}/w3/cas",
          temp_path: "{d}/w3/tmp",
          eviction_policy: {{ max_bytes: 100000000 }},
        }} }},
        slow: {{ grpc: {{
          instance_name: "main",
          endpoints: [{{ address: "grpc://127.0.0.1:{public}" }}],
          store_type: "cas",
        }} }},
        slow_direction: "get",
      }},
    }},
  ],
  schedulers: [
    {{
      name: "MAIN",
      simple: {{
        supported_platform_properties: {{ cpu_count: "minimum" }},
      }},
    }},
  ],
  workers: [
    {{ local: {{
      name: "worker-1",
      worker_api_endpoint: {{ uri: "grpc://127.0.0.1:{wapi}" }},
      cas_fast_slow_store: "W1_STORE",
      cas_server_port: {c1},
      blobs_available_interval_ms: 200,
      work_directory: "{d}/w1/work",
      upload_action_result: {{ upload_ac_results_strategy: "never" }},
      platform_properties: {{ cpu_count: {{ values: ["1"] }} }},
    }} }},
    {{ local: {{
      name: "worker-2",
      worker_api_endpoint: {{ uri: "grpc://127.0.0.1:{wapi}" }},
      cas_fast_slow_store: "W2_STORE",
      cas_server_port: {c2},
      blobs_available_interval_ms: 200,
      work_directory: "{d}/w2/work",
      upload_action_result: {{ upload_ac_results_strategy: "never" }},
      platform_properties: {{ cpu_count: {{ values: ["1"] }} }},
    }} }},
    {{ local: {{
      name: "worker-3",
      worker_api_endpoint: {{ uri: "grpc://127.0.0.1:{wapi}" }},
      cas_fast_slow_store: "W3_STORE",
      cas_server_port: {c3},
      blobs_available_interval_ms: 200,
      work_directory: "{d}/w3/work",
      upload_action_result: {{ upload_ac_results_strategy: "never" }},
      platform_properties: {{ cpu_count: {{ values: ["1"] }} }},
    }} }},
  ],
  servers: [
    {{
      name: "public",
      listener: {{ http: {{ socket_address: "127.0.0.1:{public}" }} }},
      services: {{
        cas: [{{ instance_name: "main", cas_store: "SERVER_CAS" }}],
        bytestream: [{{ instance_name: "main", cas_store: "SERVER_CAS" }}],
        capabilities: [{{ instance_name: "main", remote_execution: {{ scheduler: "MAIN" }} }}],
      }},
    }},
    {{
      name: "worker_api",
      listener: {{ http: {{ socket_address: "127.0.0.1:{wapi}" }} }},
      services: {{
        worker_api: {{ scheduler: "MAIN" }},
      }},
    }},
  ],
}}"#,
        d = d,
        wapi = ports.worker_api,
        c1 = ports.cas[0],
        c2 = ports.cas[1],
        c3 = ports.cas[2],
        public = ports.public,
    );
    let config_path = temp_dir.join("config.json5");
    std::fs::write(&config_path, config).unwrap();
    config_path
}

/// Compute SHA-256 digest of data, returning (hex_hash, size).
fn sha256_digest(data: &[u8]) -> (String, i64) {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = format!("{:x}", hasher.finalize());
    (hash, data.len() as i64)
}

/// Holds a spawned nativelink process and its collected log lines.
struct NativeLinkProcess {
    child: Child,
    log_lines: Arc<Mutex<Vec<String>>>,
    /// Set to false when stderr reader thread finishes (child exited).
    child_alive: Arc<AtomicBool>,
}

impl NativeLinkProcess {
    /// Spawn the nativelink binary with the given config file.
    fn spawn(config_path: &Path) -> Self {
        let binary = env!("CARGO_BIN_EXE_nativelink");

        let mut child = Command::new(binary)
            .arg(config_path.to_str().unwrap())
            .env(
                "RUST_LOG",
                "nativelink=trace,nativelink_worker=trace,nativelink_service=trace",
            )
            // Disable ANSI color codes for easier log parsing.
            .env("NO_COLOR", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn nativelink binary");

        let log_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let child_alive = Arc::new(AtomicBool::new(true));

        // Collect stderr lines in a background thread.
        let stderr = child.stderr.take().expect("Failed to capture stderr");
        let log_lines_stderr = log_lines.clone();
        let child_alive_stderr = child_alive.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        log_lines_stderr.lock().unwrap().push(line);
                    }
                    Err(_) => break,
                }
            }
            child_alive_stderr.store(false, Ordering::Relaxed);
        });

        // Also collect stdout in case tracing writes there.
        let stdout = child.stdout.take().expect("Failed to capture stdout");
        let log_lines_stdout = log_lines.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        log_lines_stdout.lock().unwrap().push(line);
                    }
                    Err(_) => break,
                }
            }
        });

        Self { child, log_lines, child_alive }
    }

    /// Wait until at least `count` log lines matching `pattern` appear.
    /// Returns false if the deadline expires or the child process exits.
    async fn wait_for_log_count(&self, pattern: &str, count: usize, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            {
                let lines = self.log_lines.lock().unwrap();
                let found = lines.iter().filter(|l| l.contains(pattern)).count();
                if found >= count {
                    return true;
                }
            }
            if tokio::time::Instant::now() > deadline {
                return false;
            }
            // Fail fast if the child process has exited.
            if !self.child_alive.load(Ordering::Relaxed) {
                // Give a brief moment for final log lines to flush.
                tokio::time::sleep(Duration::from_millis(200)).await;
                let lines = self.log_lines.lock().unwrap();
                let found = lines.iter().filter(|l| l.contains(pattern)).count();
                if found < count {
                    eprintln!(
                        "!!! Child process exited while waiting for pattern={:?} count={} (found {}). Last 30 lines:",
                        pattern, count, found,
                    );
                    for line in lines.iter().rev().take(30).collect::<Vec<_>>().into_iter().rev() {
                        eprintln!("  {line}");
                    }
                }
                return found >= count;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Count how many log lines match `pattern`.
    fn count_logs(&self, pattern: &str) -> usize {
        let lines = self.log_lines.lock().unwrap();
        lines.iter().filter(|l| l.contains(pattern)).count()
    }

    /// Get all log lines matching `pattern`.
    fn grep_logs(&self, pattern: &str) -> Vec<String> {
        let lines = self.log_lines.lock().unwrap();
        lines
            .iter()
            .filter(|l| l.contains(pattern))
            .cloned()
            .collect()
    }
}

impl Drop for NativeLinkProcess {
    fn drop(&mut self) {
        // Send SIGKILL to stop the process.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Upload a blob to a worker's CAS endpoint via BatchUpdateBlobs.
async fn upload_blob_to_worker_cas(
    port: u16,
    data: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let channel = Channel::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .connect()
        .await?;

    let mut client = ContentAddressableStorageClient::new(channel);

    let (hash, size) = sha256_digest(data);

    let request = BatchUpdateBlobsRequest {
        instance_name: String::new(),
        requests: vec![batch_update_blobs_request::Request {
            digest: Some(Digest {
                hash,
                size_bytes: size,
            }),
            data: data.to_vec().into(),
            compressor: 0,
        }],
        digest_function: 0, // SHA256
    };

    client.batch_update_blobs(request).await?;
    Ok(())
}

/// Read a blob from a CAS endpoint via BatchReadBlobs.
/// Returns Ok(data) on success, or Err on gRPC/transport error.
/// A gRPC OK with a non-OK status in the response means the blob was not found.
async fn read_blob_from_cas(
    port: u16,
    instance_name: &str,
    hash: &str,
    size: i64,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    let channel = Channel::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .connect()
        .await?;

    let mut client = ContentAddressableStorageClient::new(channel);

    let request = BatchReadBlobsRequest {
        instance_name: instance_name.to_string(),
        digests: vec![Digest {
            hash: hash.to_string(),
            size_bytes: size,
        }],
        acceptable_compressors: vec![],
        digest_function: 0,
    };

    let response = client.batch_read_blobs(request).await?;
    let inner = response.into_inner();

    if let Some(resp) = inner.responses.first() {
        // status code 0 = OK
        if resp.status.as_ref().is_some_and(|s| s.code == 0) {
            return Ok(Some(resp.data.to_vec()));
        }
    }
    Ok(None)
}

/// Represents a per-digest result from BatchReadBlobs.
#[allow(dead_code)]
struct CasReadResult {
    /// gRPC status code (0 = OK, 14 = Unavailable, 5 = NotFound, etc.)
    code: i32,
    /// Status message (may contain redirect prefix for worker requests).
    message: String,
    /// Blob data (empty if not OK).
    data: Vec<u8>,
}

/// Read a blob from a CAS endpoint with the `x-nativelink-worker` header set,
/// simulating a worker-to-server request. Returns the raw per-digest result.
async fn read_blob_from_cas_as_worker(
    port: u16,
    instance_name: &str,
    hash: &str,
    size: i64,
) -> Result<CasReadResult, Box<dyn std::error::Error>> {
    let channel = Channel::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .connect()
        .await?;

    let mut client = ContentAddressableStorageClient::new(channel);

    let mut request = tonic::Request::new(BatchReadBlobsRequest {
        instance_name: instance_name.to_string(),
        digests: vec![Digest {
            hash: hash.to_string(),
            size_bytes: size,
        }],
        acceptable_compressors: vec![],
        digest_function: 0,
    });
    // Mark this as a worker request so the server returns a redirect
    // instead of proxying the blob data.
    request
        .metadata_mut()
        .insert("x-nativelink-worker", MetadataValue::from_static("true"));

    let response = client.batch_read_blobs(request).await?;
    let inner = response.into_inner();

    let resp = inner
        .responses
        .into_iter()
        .next()
        .expect("Expected at least one response");
    let status = resp.status.unwrap_or_default();
    Ok(CasReadResult {
        code: status.code,
        message: status.message,
        data: resp.data.to_vec(),
    })
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// Verify the full BlobsAvailable pipeline with 3 workers.
///
/// Steps:
/// 1. Start a nativelink server with 3 workers, each with a CAS port
/// 2. Wait for all workers to register and start BlobsAvailable reporting
/// 3. Verify that each worker sends an initial full-snapshot BlobsAvailable
/// 4. Upload unique blobs to each worker's CAS endpoint
/// 5. Wait for the next periodic tick to send a delta BlobsAvailable
/// 6. Verify the server logs show the blobs being registered in the locality map
/// 7. Shutdown and verify cleanup
#[tokio::test(flavor = "multi_thread")]
async fn test_blobs_available_three_workers() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ports = allocate_ports();
    let config_path = write_config(temp_dir.path(), &ports);

    // --- Phase 1: Start the server ---

    let process = NativeLinkProcess::spawn(&config_path);

    // Wait for both server listeners to be ready.
    let startup_timeout = Duration::from_secs(30);
    assert!(
        process
            .wait_for_log_count("Ready, listening on", 2, startup_timeout)
            .await,
        "Server did not start both listeners within timeout. \
         Lines captured: {}. Last 20 lines:\n{}",
        process.log_lines.lock().unwrap().len(),
        {
            let lines = process.log_lines.lock().unwrap();
            lines.iter().rev().take(20).rev().cloned().collect::<Vec<_>>().join("\n")
        },
    );


    // --- Phase 2: Wait for all 3 workers to connect ---
    assert!(
        process
            .wait_for_log_count("Worker registered with scheduler", 3, Duration::from_secs(15))
            .await,
        "Not all 3 workers registered. Found {} registrations. Logs:\n{}",
        process.count_logs("Worker registered with scheduler"),
        process.grep_logs("Worker registered").join("\n"),
    );

    // --- Phase 3: Verify BlobsAvailable reporting was registered ---
    assert!(
        process
            .wait_for_log_count(
                "Registered periodic BlobsAvailable reporting",
                3,
                Duration::from_secs(5),
            )
            .await,
        "Not all 3 workers registered BlobsAvailable callbacks. Found {}.",
        process.count_logs("Registered periodic BlobsAvailable reporting"),
    );

    // --- Phase 4: Wait for initial full-snapshot BlobsAvailable ---
    // Each worker sends a full snapshot (is_first=true) on the first periodic tick.
    // blobs_available_interval_ms=200, so this should happen within ~1 second.
    assert!(
        process
            .wait_for_log_count("Sent periodic BlobsAvailable", 3, Duration::from_secs(5))
            .await,
        "Not all 3 workers sent initial BlobsAvailable. Found {}.",
        process.count_logs("Sent periodic BlobsAvailable"),
    );

    // Verify that the initial snapshots had is_first=true.
    let initial_logs = process.grep_logs("Sent periodic BlobsAvailable");
    let is_first_count = initial_logs.iter().filter(|l| l.contains("is_first=true") || l.contains("is_first: true")).count();
    assert!(
        is_first_count >= 3,
        "Expected at least 3 is_first=true BlobsAvailable, found {is_first_count}. Logs:\n{}",
        initial_logs.join("\n"),
    );


    // --- Phase 5: Upload blobs to each worker's CAS ---
    // Capture the send count BEFORE uploads so we can detect new delta sends.
    let before_upload_send_count = process.count_logs("Sent periodic BlobsAvailable");
    let blob_data: Vec<Vec<u8>> = vec![
        b"Hello from worker-1! This is test blob data.".to_vec(),
        b"Hello from worker-2! Different test blob data.".to_vec(),
        b"Hello from worker-3! Yet another test blob.".to_vec(),
    ];

    for (i, data) in blob_data.iter().enumerate() {
        let port = ports.cas[i];
        // Retry a few times in case the worker CAS server isn't ready yet.
        let mut uploaded = false;
        for _ in 0..10 {
            match upload_blob_to_worker_cas(port, data).await {
                Ok(()) => {
                    uploaded = true;
                    break;
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
        assert!(uploaded, "Failed to upload blob to worker-{}", i + 1);
    }

    // --- Phase 6: Wait for delta BlobsAvailable with the new blobs ---
    // After uploading, the BlobChangeTracker's on_insert callback fires.
    // The next periodic tick (within 200ms) will send a delta.
    // We captured before_upload_send_count before uploads started.
    assert!(
        process
            .wait_for_log_count(
                "Sent periodic BlobsAvailable",
                before_upload_send_count + 3,
                Duration::from_secs(5),
            )
            .await,
        "Workers did not send delta BlobsAvailable after blob upload. \
         Had {before_upload_send_count} sends before upload, now have {}.",
        process.count_logs("Sent periodic BlobsAvailable"),
    );

    // --- Phase 7: Verify server-side logging ---
    // The WorkerApiServer should log "Registering blobs available from worker"
    // for both the initial snapshot and the delta.
    let server_register_count = process.count_logs("Registering blobs available from worker");
    assert!(
        server_register_count >= 3,
        "Expected at least 3 'Registering blobs available from worker' logs, found {server_register_count}.",
    );

    // --- Phase 8: Verify delta-specific behavior ---
    // After the initial full snapshot, subsequent sends should be deltas.
    let all_sends = process.grep_logs("Sent periodic BlobsAvailable");
    let delta_sends = all_sends
        .iter()
        .filter(|l| l.contains("is_first=false") || l.contains("is_first: false"))
        .count();
    assert!(
        delta_sends >= 3,
        "Expected at least 3 delta BlobsAvailable sends (is_first=false), found {delta_sends}.",
    );


    // --- Phase 10: Verify no-change ticks are skipped (trace level) ---
    // Workers that have no changes since last tick should log
    // "BlobsAvailable: no changes since last tick, skipping" at trace level.
    // Give a little extra time for ticks with no changes.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let skip_count = process.count_logs("no changes since last tick, skipping");
    // We expect at least some skips once the delta has been sent and there
    // are no further changes.
    assert!(
        skip_count > 0,
        "Expected at least some 'no changes since last tick, skipping' trace logs \
         (workers should skip sending when there are no new changes).",
    );

    // --- Phase 11: Verify the starting CAS server logs ---
    let cas_server_logs = process.grep_logs("Starting worker CAS TCP server for peer blob sharing");
    assert_eq!(
        cas_server_logs.len(),
        3,
        "Expected 3 worker CAS server start logs, found {}. Logs:\n{}",
        cas_server_logs.len(),
        cas_server_logs.join("\n"),
    );


    // --- Phase 12: Worker-2 reads blob from Worker-1 via peer sharing ---
    // Upload a unique blob to Worker-1's CAS only. After BlobsAvailable
    // propagates to the server's locality map, Worker-2 can fetch the blob
    // through the chain: Worker-2 CAS → slow store (GrpcStore → server) →
    // server WorkerProxyStore → locality map → Worker-1 CAS.
    let cross_worker_blob = b"cross-worker test blob for peer sharing";
    let (cw_hash, cw_size) = sha256_digest(cross_worker_blob);

    // Capture count BEFORE the upload so the delta is not missed.
    let before_register = process.count_logs("Registering blobs available from worker");

    // Upload to Worker-1's CAS.
    upload_blob_to_worker_cas(ports.cas[0], cross_worker_blob)
        .await
        .expect("Failed to upload cross-worker blob to worker-1");

    // Read the blob back from Worker-1's CAS — should succeed directly.
    let data = read_blob_from_cas(ports.cas[0], "", &cw_hash, cw_size)
        .await
        .expect("gRPC read from worker-1 failed");
    assert_eq!(
        data.as_deref(),
        Some(cross_worker_blob.as_slice()),
        "Blob read from worker-1's CAS should match uploaded data",
    );
    assert!(
        process
            .wait_for_log_count(
                "Registering blobs available from worker",
                before_register + 1,
                Duration::from_secs(5),
            )
            .await,
        "Server did not register BlobsAvailable after cross-worker blob upload.",
    );

    // Now read from Worker-2's CAS — Worker-2 doesn't have the blob locally,
    // so its effective_cas_store chain kicks in:
    //   fast (FilesystemStore) miss → slow (WorkerProxyStore(GrpcStore → server))
    //   → server redirects → WorkerProxyStore follows redirect → Worker-1 → success
    let data = read_blob_from_cas(ports.cas[1], "", &cw_hash, cw_size)
        .await
        .expect("gRPC read from worker-2 failed");

    assert_eq!(
        data.as_deref(),
        Some(cross_worker_blob.as_slice()),
        "Worker-2 should fetch the blob from Worker-1 via peer sharing",
    );

    // --- Phase 13: Server proxies CAS read to a worker ---
    // The server's CAS (SERVER_CAS) is an empty MemoryStore wrapped with
    // WorkerProxyStore. When a blob is not found locally, WorkerProxyStore
    // consults the server-side locality map (populated by BlobsAvailable)
    // and proxies the read to the worker that has it.

    // Upload a unique blob to Worker-3's CAS.
    let proxy_blob = b"proxy test blob - only on worker-3";
    let (px_hash, px_size) = sha256_digest(proxy_blob);

    // Capture count BEFORE the upload so the delta is not missed.
    let before_register = process.count_logs("Registering blobs available from worker");

    upload_blob_to_worker_cas(ports.cas[2], proxy_blob)
        .await
        .expect("Failed to upload proxy blob to worker-3");
    assert!(
        process
            .wait_for_log_count(
                "Registering blobs available from worker",
                before_register + 1,
                Duration::from_secs(5),
            )
            .await,
        "Server did not register new BlobsAvailable after proxy blob upload.",
    );

    // Now read the blob via the server's public CAS endpoint.
    // The server's MemoryStore doesn't have it, so WorkerProxyStore should
    // proxy the read to Worker-3's CAS.
    let data = read_blob_from_cas(ports.public, "main", &px_hash, px_size)
        .await
        .expect("gRPC read from server failed");

    assert_eq!(
        data.as_deref(),
        Some(proxy_blob.as_slice()),
        "Server should proxy the CAS read to worker-3 and return the blob",
    );

    // Verify the WorkerProxyStore logged the proxy operation.
    assert!(
        process
            .wait_for_log_count(
                "WorkerProxyStore: successfully proxied blob from worker",
                1,
                Duration::from_secs(3),
            )
            .await,
        "Expected WorkerProxyStore to log successful proxy read. Logs:\n{}",
        process
            .grep_logs("WorkerProxyStore")
            .join("\n"),
    );

    // --- Phase 14: Verify proxy vs redirect behavior ---
    // Non-worker requests to the server's CAS should get proxied data.
    // Worker requests (with x-nativelink-worker header) should get a redirect.

    // Upload a fresh blob to Worker-1 for this test.
    let redirect_blob = b"redirect vs proxy test blob - only on worker-1";
    let (rd_hash, rd_size) = sha256_digest(redirect_blob);

    // Capture count BEFORE the upload so the delta is not missed.
    let before_register = process.count_logs("Registering blobs available from worker");

    upload_blob_to_worker_cas(ports.cas[0], redirect_blob)
        .await
        .expect("Failed to upload redirect test blob to worker-1");
    assert!(
        process
            .wait_for_log_count(
                "Registering blobs available from worker",
                before_register + 1,
                Duration::from_secs(5),
            )
            .await,
        "Server did not register BlobsAvailable for redirect test blob.",
    );

    // 14a: Worker request → server returns redirect with peer endpoints.
    // Must run before the non-worker proxy test, because proxying caches
    // the blob in the server's inner store (get_part_and_cache), which
    // would make the redirect test succeed with code 0 instead of 9.
    let result = read_blob_from_cas_as_worker(ports.public, "main", &rd_hash, rd_size)
        .await
        .expect("Worker read from server failed at transport level");
    // The server should return FailedPrecondition (code 9) with NL_REDIRECT:
    // prefix containing the worker endpoint(s) that have the blob.
    // FailedPrecondition is used instead of Unavailable so the GrpcStore
    // retrier does not waste time retrying what is actually a redirect.
    assert_eq!(
        result.code, 9, // Code::FailedPrecondition
        "Worker request should get FailedPrecondition redirect, got code={} message={:?}",
        result.code, result.message,
    );
    assert!(
        result.message.contains("NL_REDIRECT:"),
        "Worker redirect message should contain NL_REDIRECT: prefix, got: {:?}",
        result.message,
    );
    // The redirect should contain Worker-1's CAS endpoint.
    // Workers advertise as grpc://<hostname>:<port>, so check for the port.
    let expected_port_suffix = format!(":{}", ports.cas[0]);
    assert!(
        result.message.contains(&expected_port_suffix),
        "Redirect should contain worker-1's CAS port ({}), got: {:?}",
        expected_port_suffix, result.message,
    );

    // 14b: Non-worker request → server proxies data back (and caches it).
    let data = read_blob_from_cas(ports.public, "main", &rd_hash, rd_size)
        .await
        .expect("Non-worker read from server failed");
    assert_eq!(
        data.as_deref(),
        Some(redirect_blob.as_slice()),
        "Non-worker request should get proxied blob data from the server",
    );

    // --- Phase 15: Multi-worker redirect lists all endpoints ---
    // Upload a blob to Worker-1, then read it from Worker-2 (which populates
    // Worker-2's CAS via the peer fetch). After Worker-2's BlobsAvailable
    // propagates, a worker request to the server should get a redirect
    // listing BOTH Worker-1 and Worker-2 as endpoints.
    let multi_blob = b"multi-redirect test blob for phase 15";
    let (multi_hash, multi_size) = sha256_digest(multi_blob);

    let before_register = process.count_logs("Registering blobs available from worker");

    // Upload to Worker-1.
    upload_blob_to_worker_cas(ports.cas[0], multi_blob)
        .await
        .expect("Failed to upload multi-redirect blob to worker-1");

    // Wait for the server to register the blob from Worker-1.
    assert!(
        process
            .wait_for_log_count(
                "Registering blobs available from worker",
                before_register + 1,
                Duration::from_secs(5),
            )
            .await,
        "Server did not register BlobsAvailable for multi-redirect blob.",
    );

    let before_register = process.count_logs("Registering blobs available from worker");

    // Read from Worker-2's CAS — this triggers peer fetch from Worker-1,
    // populating Worker-2's local CAS.
    let data = read_blob_from_cas(ports.cas[1], "", &multi_hash, multi_size)
        .await
        .expect("Worker-2 peer fetch failed for multi-redirect blob");
    assert_eq!(
        data.as_deref(),
        Some(multi_blob.as_slice()),
        "Worker-2 should fetch multi-redirect blob from Worker-1",
    );

    // Wait for Worker-2's BlobsAvailable to propagate the newly cached blob.
    assert!(
        process
            .wait_for_log_count(
                "Registering blobs available from worker",
                before_register + 1,
                Duration::from_secs(5),
            )
            .await,
        "Server did not register Worker-2's BlobsAvailable after peer fetch.",
    );

    // Now a worker request should get a redirect listing BOTH workers.
    let result = read_blob_from_cas_as_worker(ports.public, "main", &multi_hash, multi_size)
        .await
        .expect("Worker read for multi-redirect failed");
    assert_eq!(
        result.code, 9,
        "Multi-redirect should use FailedPrecondition, got code={} message={:?}",
        result.code, result.message,
    );
    assert!(
        result.message.contains("NL_REDIRECT:"),
        "Multi-redirect should contain NL_REDIRECT: prefix, got: {:?}",
        result.message,
    );
    // Both Worker-1 and Worker-2 CAS ports should appear in the redirect.
    let w1_port = format!(":{}", ports.cas[0]);
    let w2_port = format!(":{}", ports.cas[1]);
    assert!(
        result.message.contains(&w1_port) && result.message.contains(&w2_port),
        "Redirect should list both worker-1 ({}) and worker-2 ({}), got: {:?}",
        w1_port, w2_port, result.message,
    );

    // Process is killed on drop.
}
