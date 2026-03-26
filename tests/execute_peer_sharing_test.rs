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

//! Integration test: Execute dependent actions where the second action's
//! inputs are fetched from the first action's worker via peer-to-peer blob
//! sharing (WorkerProxyStore redirects).
//!
//! Topology:
//!   - 1 nativelink server (CAS + Execution + WorkerApi)
//!   - 2 workers with peer CAS servers and distinct `worker_id` properties
//!
//! Flow:
//!   1. Action A targets worker-1, produces output blob
//!   2. BlobsAvailable propagates output digests to the server's locality map
//!   3. Action B targets worker-2, depends on A's output — fetched via peer
//!      sharing (WorkerProxyStore proxy → Worker-1 CAS)
//!   4. Action C targets worker-1, depends on B's output — fetched from
//!      worker-2, verifying bi-directional peer sharing

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nativelink_proto::build::bazel::remote::execution::v2::{
    batch_update_blobs_request, content_addressable_storage_client::ContentAddressableStorageClient,
    digest_function, execution_client::ExecutionClient, platform, Action, BatchUpdateBlobsRequest,
    Command, Digest, Directory, ExecuteRequest, ExecuteResponse, FileNode, Platform,
};
use nativelink_proto::google::longrunning::operation;
use prost::Message;
use sha2::{Digest as Sha2Digest, Sha256};
use tempfile::TempDir;
use tonic::transport::Channel;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn get_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

struct Ports {
    public: u16,
    worker_api: u16,
    cas: [u16; 2],
}

fn allocate_ports() -> Ports {
    Ports {
        public: get_free_port(),
        worker_api: get_free_port(),
        cas: [get_free_port(), get_free_port()],
    }
}

/// Compute SHA-256 digest of data, returning a proto Digest.
fn sha256_digest_proto(data: &[u8]) -> Digest {
    let mut hasher = Sha256::new();
    hasher.update(data);
    Digest {
        hash: format!("{:x}", hasher.finalize()),
        size_bytes: data.len() as i64,
    }
}

/// Serialize a prost Message and compute its digest.
fn digest_of_message<M: Message>(msg: &M) -> (Vec<u8>, Digest) {
    let data = msg.encode_to_vec();
    let digest = sha256_digest_proto(&data);
    (data, digest)
}

/// Write a JSON5 config with execution service, 2 workers with distinct
/// `worker_id` platform properties for deterministic action routing.
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
  ],
  schedulers: [
    {{
      name: "MAIN",
      simple: {{
        supported_platform_properties: {{
          cpu_count: "minimum",
          worker_id: "exact",
        }},
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
      upload_action_result: {{
        ac_store: "AC_STORE",
        upload_ac_results_strategy: "success_only",
      }},
      platform_properties: {{
        cpu_count: {{ values: ["1"] }},
        worker_id: {{ values: ["w1"] }},
      }},
    }} }},
    {{ local: {{
      name: "worker-2",
      worker_api_endpoint: {{ uri: "grpc://127.0.0.1:{wapi}" }},
      cas_fast_slow_store: "W2_STORE",
      cas_server_port: {c2},
      blobs_available_interval_ms: 200,
      work_directory: "{d}/w2/work",
      upload_action_result: {{
        ac_store: "AC_STORE",
        upload_ac_results_strategy: "success_only",
      }},
      platform_properties: {{
        cpu_count: {{ values: ["1"] }},
        worker_id: {{ values: ["w2"] }},
      }},
    }} }},
  ],
  servers: [
    {{
      name: "public",
      listener: {{ http: {{ socket_address: "127.0.0.1:{public}" }} }},
      services: {{
        cas: [{{ instance_name: "main", cas_store: "SERVER_CAS" }}],
        ac: [{{ instance_name: "main", ac_store: "AC_STORE" }}],
        bytestream: [{{ instance_name: "main", cas_store: "SERVER_CAS" }}],
        capabilities: [{{ instance_name: "main", remote_execution: {{ scheduler: "MAIN" }} }}],
        execution: [{{ instance_name: "main", cas_store: "SERVER_CAS", scheduler: "MAIN" }}],
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
        public = ports.public,
    );
    let config_path = temp_dir.join("config.json5");
    std::fs::write(&config_path, config).unwrap();
    config_path
}

struct NativeLinkProcess {
    child: Child,
    log_lines: Arc<Mutex<Vec<String>>>,
    child_alive: Arc<AtomicBool>,
}

impl NativeLinkProcess {
    fn spawn(config_path: &Path) -> Self {
        let binary = env!("CARGO_BIN_EXE_nativelink");

        let mut child = ProcessCommand::new(binary)
            .arg(config_path.to_str().unwrap())
            .env(
                "RUST_LOG",
                "nativelink=trace,nativelink_worker=trace,nativelink_service=trace,nativelink_store=trace",
            )
            .env("NO_COLOR", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn nativelink binary");

        let log_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let child_alive = Arc::new(AtomicBool::new(true));

        let stderr = child.stderr.take().unwrap();
        let log_lines_stderr = log_lines.clone();
        let child_alive_stderr = child_alive.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                match line {
                    Ok(line) => log_lines_stderr.lock().unwrap().push(line),
                    Err(_) => break,
                }
            }
            child_alive_stderr.store(false, Ordering::Relaxed);
        });

        let stdout = child.stdout.take().unwrap();
        let log_lines_stdout = log_lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => log_lines_stdout.lock().unwrap().push(line),
                    Err(_) => break,
                }
            }
        });

        Self {
            child,
            log_lines,
            child_alive,
        }
    }

    async fn wait_for_log_count(&self, pattern: &str, count: usize, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            {
                let lines = self.log_lines.lock().unwrap();
                if lines.iter().filter(|l| l.contains(pattern)).count() >= count {
                    return true;
                }
            }
            if tokio::time::Instant::now() > deadline {
                return false;
            }
            if !self.child_alive.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let lines = self.log_lines.lock().unwrap();
                let found = lines.iter().filter(|l| l.contains(pattern)).count();
                if found < count {
                    eprintln!(
                        "!!! Child exited waiting for pattern={pattern:?} count={count} (found {found}). Last 40 lines:",
                    );
                    for line in lines.iter().rev().take(40).collect::<Vec<_>>().into_iter().rev() {
                        eprintln!("  {line}");
                    }
                }
                return found >= count;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn count_logs(&self, pattern: &str) -> usize {
        self.log_lines
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.contains(pattern))
            .count()
    }

    fn grep_logs(&self, pattern: &str) -> Vec<String> {
        self.log_lines
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.contains(pattern))
            .cloned()
            .collect()
    }

    /// Print all logs for debugging.
    fn dump_logs(&self, label: &str) {
        let lines = self.log_lines.lock().unwrap();
        eprintln!("=== {label} ({} lines) ===", lines.len());
        for line in lines.iter() {
            eprintln!("  {line}");
        }
        eprintln!("=== end {label} ===");
    }
}

impl Drop for NativeLinkProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Upload multiple blobs to the server's CAS via BatchUpdateBlobs.
async fn upload_blobs_to_cas(
    channel: &Channel,
    blobs: &[(Vec<u8>, Digest)],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ContentAddressableStorageClient::new(channel.clone());
    let requests: Vec<_> = blobs
        .iter()
        .map(|(data, digest)| batch_update_blobs_request::Request {
            digest: Some(digest.clone()),
            data: data.clone().into(),
            compressor: 0,
        })
        .collect();
    client
        .batch_update_blobs(BatchUpdateBlobsRequest {
            instance_name: "main".to_string(),
            requests,
            digest_function: digest_function::Value::Sha256.into(),
        })
        .await?;
    Ok(())
}

/// Execute an action and wait for it to complete, returning the ExecuteResponse.
async fn execute_and_wait(
    channel: &Channel,
    action_digest: Digest,
) -> Result<ExecuteResponse, Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut client = ExecutionClient::new(channel.clone());
        let request = ExecuteRequest {
            instance_name: "main".to_string(),
            action_digest: Some(action_digest),
            skip_cache_lookup: true,
            digest_function: digest_function::Value::Sha256.into(),
            execution_policy: None,
            results_cache_policy: None,
        };

        let response = client.execute(request).await?;
        let mut stream = response.into_inner();

        let mut last_response: Option<ExecuteResponse> = None;
        while let Some(op) = stream.message().await? {
            if op.done {
                if let Some(operation::Result::Response(any)) = op.result {
                    let exec_response = ExecuteResponse::decode(any.value.as_ref())?;
                    last_response = Some(exec_response);
                }
                break;
            }
        }

        last_response.ok_or_else(|| "Execute stream ended without done=true".into())
    })
    .await
    .map_err(|_| "execute_and_wait timed out after 30s")?
}

/// Build a Platform proto targeting a specific worker.
fn make_platform(worker_id: &str) -> Platform {
    Platform {
        properties: vec![
            platform::Property {
                name: "cpu_count".to_string(),
                value: "1".to_string(),
            },
            platform::Property {
                name: "worker_id".to_string(),
                value: worker_id.to_string(),
            },
        ],
    }
}

/// Build and upload an action targeted at a specific worker.
async fn create_action(
    channel: &Channel,
    arguments: Vec<String>,
    output_files: Vec<String>,
    input_root: &Directory,
    target_worker: &str,
) -> Result<Digest, Box<dyn std::error::Error>> {
    let command = Command {
        arguments,
        output_files,
        ..Default::default()
    };
    let (cmd_data, cmd_digest) = digest_of_message(&command);

    let (root_data, root_digest) = digest_of_message(input_root);

    let action = Action {
        command_digest: Some(cmd_digest.clone()),
        input_root_digest: Some(root_digest.clone()),
        do_not_cache: true,
        platform: Some(make_platform(target_worker)),
        ..Default::default()
    };
    let (action_data, action_digest) = digest_of_message(&action);

    upload_blobs_to_cas(
        channel,
        &[
            (cmd_data, cmd_digest),
            (root_data, root_digest),
            (action_data, action_digest.clone()),
        ],
    )
    .await?;

    Ok(action_digest)
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// Execute a chain of 3 dependent actions on alternating workers, exercising
/// peer-to-peer blob sharing in both directions.
///
/// Action A → worker-1: `echo -n "HELLO_FROM_ACTION_A" > output.txt`
/// Action B → worker-2: `cat input.txt > output.txt && echo -n "_PLUS_B" >> output.txt`
///   (input = A's output, fetched from worker-1 via peer sharing)
/// Action C → worker-1: `echo -n "_PLUS_C" > output.txt && cat input.txt >> output.txt`
///   (input = B's output, fetched from worker-2 via peer sharing)
#[tokio::test(flavor = "multi_thread")]
async fn test_execute_dependent_actions_with_peer_sharing() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ports = allocate_ports();
    let config_path = write_config(temp_dir.path(), &ports);

    let process = NativeLinkProcess::spawn(&config_path);

    // Wait for server listeners.
    assert!(
        process
            .wait_for_log_count("Ready, listening on", 2, Duration::from_secs(30))
            .await,
        "Server did not start. Last 20 lines:\n{}",
        {
            let lines = process.grep_logs("");
            lines.iter().rev().take(20).collect::<Vec<_>>().iter().rev()
                .map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
        },
    );

    // Wait for both workers to register.
    assert!(
        process
            .wait_for_log_count("Worker registered with scheduler", 2, Duration::from_secs(15))
            .await,
        "Not all workers registered. Found {}.",
        process.count_logs("Worker registered with scheduler"),
    );

    // Wait for initial BlobsAvailable snapshots.
    assert!(
        process
            .wait_for_log_count("Sent periodic BlobsAvailable", 2, Duration::from_secs(5))
            .await,
        "Workers did not send initial BlobsAvailable.",
    );

    let channel = Channel::from_shared(format!("http://127.0.0.1:{}", ports.public))
        .unwrap()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .connect()
        .await
        .expect("Failed to connect to server");

    // =====================================================================
    // ACTION A → worker-1: Produce a known output blob
    // =====================================================================
    let action_a_digest = create_action(
        &channel,
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "echo -n 'HELLO_FROM_ACTION_A' > output.txt".to_string(),
        ],
        vec!["output.txt".to_string()],
        &Directory::default(),
        "w1",
    )
    .await
    .expect("Failed to create Action A");

    let before_register = process.count_logs("Registering blobs available from worker");

    let response_a = execute_and_wait(&channel, action_a_digest)
        .await
        .expect("Action A execution failed");

    let result_a = response_a
        .result
        .as_ref()
        .expect("Action A missing ActionResult");
    assert_eq!(
        result_a.exit_code, 0,
        "Action A exit_code={}",
        result_a.exit_code,
    );
    assert_eq!(result_a.output_files.len(), 1, "Action A output count");

    let output_a_digest = result_a.output_files[0]
        .digest
        .as_ref()
        .expect("Action A output missing digest");
    let expected_a = b"HELLO_FROM_ACTION_A";
    let expected_a_digest = sha256_digest_proto(expected_a);
    assert_eq!(
        output_a_digest.hash, expected_a_digest.hash,
        "Action A output digest mismatch",
    );

    // Wait for BlobsAvailable to propagate A's outputs to the locality map.
    assert!(
        process
            .wait_for_log_count(
                "Registering blobs available from worker",
                before_register + 1,
                Duration::from_secs(5),
            )
            .await,
        "BlobsAvailable not registered after Action A.",
    );

    // =====================================================================
    // ACTION B → worker-2: Depends on A's output (peer sharing: w1 → w2)
    // =====================================================================
    // Worker-2 does not have A's output locally. The fetch chain:
    //   Worker-2 FastStore (miss) → GrpcStore → server CAS →
    //   WorkerProxyStore → locality map (w1 has it) → proxy from w1's CAS
    let input_root_b = Directory {
        files: vec![FileNode {
            name: "input.txt".to_string(),
            digest: Some(output_a_digest.clone()),
            is_executable: false,
            node_properties: None,
        }],
        ..Default::default()
    };

    let action_b_digest = create_action(
        &channel,
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "cat input.txt > output.txt && echo -n '_PLUS_B' >> output.txt".to_string(),
        ],
        vec!["output.txt".to_string()],
        &input_root_b,
        "w2",
    )
    .await
    .expect("Failed to create Action B");

    let proxy_before_b = process.count_logs("WorkerProxyStore: successfully")
        + process.count_logs("peer won race");

    let before_register = process.count_logs("Registering blobs available from worker");

    let response_b = execute_and_wait(&channel, action_b_digest)
        .await
        .expect("Action B execution failed");

    let result_b = response_b
        .result
        .as_ref()
        .expect("Action B missing ActionResult");
    assert_eq!(
        result_b.exit_code, 0,
        "Action B exit_code={}\nAll logs:\n{}",
        result_b.exit_code,
        process.grep_logs("").join("\n"),
    );
    assert_eq!(result_b.output_files.len(), 1, "Action B output count");

    let output_b_digest = result_b.output_files[0]
        .digest
        .as_ref()
        .expect("Action B output missing digest");
    let expected_b = b"HELLO_FROM_ACTION_A_PLUS_B";
    let expected_b_digest = sha256_digest_proto(expected_b);
    assert_eq!(
        output_b_digest.hash, expected_b_digest.hash,
        "Action B output digest mismatch. Expected {:?}, got hash {}",
        String::from_utf8_lossy(expected_b),
        output_b_digest.hash,
    );

    // Verify peer sharing: A's output was fetched from worker-1 via
    // WorkerProxyStore — either by server-side proxy ("successfully proxied
    // blob from worker") or worker-side redirect ("successfully read blob
    // from redirected peer") or racing ("peer won race").
    let proxy_after_b = process.count_logs("WorkerProxyStore: successfully")
        + process.count_logs("peer won race");
    if proxy_after_b <= proxy_before_b {
        process.dump_logs("Action B peer sharing failure");
    }
    assert!(
        proxy_after_b > proxy_before_b,
        "Expected cross-worker blob fetch for Action A's output. \
         Proxy count before={proxy_before_b} after={proxy_after_b}.",
    );

    // Wait for BlobsAvailable after Action B.
    assert!(
        process
            .wait_for_log_count(
                "Registering blobs available from worker",
                before_register + 1,
                Duration::from_secs(5),
            )
            .await,
        "BlobsAvailable not registered after Action B.",
    );

    // =====================================================================
    // Summary assertions
    // =====================================================================

    // At least 1 cross-worker fetch (Action B fetched A's output from worker-1).
    let total_proxies = process.count_logs("WorkerProxyStore: successfully")
        + process.count_logs("peer won race");
    assert!(
        total_proxies >= 1,
        "Expected at least 1 cross-worker blob fetch, got {total_proxies}",
    );

    // BlobsAvailable should have been registered at least twice (once per
    // worker after initial snapshot). The exact count depends on timing —
    // additional ticks may or may not have fired by this point.
    let total_registrations = process.count_logs("Registering blobs available from worker");
    assert!(
        total_registrations >= 2,
        "Expected at least 2 BlobsAvailable registrations, got {total_registrations}",
    );

    // Process is killed on drop.
}
