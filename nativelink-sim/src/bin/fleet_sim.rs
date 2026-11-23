// Copyright 2026 Trace Machina, Inc. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! `fleet-sim` (RFC-0061): simulated workers against a real scheduler, and
//! a queue of synthetic actions through real `Execute` calls.

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use clap::{Parser, Subcommand};
use futures::StreamExt;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::batch_update_blobs_request::Request as BlobRequest;
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_client::ContentAddressableStorageClient;
use nativelink_proto::build::bazel::remote::execution::v2::execution_client::ExecutionClient;
use nativelink_proto::build::bazel::remote::execution::v2::{
    Action, BatchUpdateBlobsRequest, Command, Digest, Directory, ExecuteRequest, ExecuteResponse,
    WaitExecutionRequest, digest_function,
};
use nativelink_proto::google::longrunning::operation;
use nativelink_sim::cas_load::{self, CasLoad};
use nativelink_sim::fleet::{Counters, Distribution, FleetWorker, spawn_worker};
use nativelink_sim::property::Rng;
use nativelink_util::background_spawn;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use prost::Message;
use tokio::sync::Semaphore;
use tokio::time::Instant;
use tonic::transport::Channel;

#[derive(Parser, Debug)]
#[command(about = "Simulated workers and a synthetic queue against a real NativeLink")]
struct Args {
    #[command(subcommand)]
    command: Command_,
}

#[derive(Subcommand, Debug)]
enum Command_ {
    /// Registers simulated workers with a scheduler's worker API.
    Workers(WorkersArgs),
    /// Uploads distinct actions and executes them, reading every stream to
    /// the end.
    Queue(QueueArgs),
    /// Drives the CAS budgets of the L point against one CAS endpoint and
    /// checks every answer.
    Cas(CasArgs),
}

#[derive(clap::Args, Debug)]
struct CasArgs {
    /// The CAS endpoint, e.g. `grpc://127.0.0.1:50051`.
    #[arg(long)]
    cas: String,
    #[arg(long, default_value = "main")]
    instance: String,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Digests in one `FindMissingBlobs` call.
    #[arg(long, default_value_t = 50_000)]
    find_missing_digests: usize,
    /// Blobs written at once through `ByteStream`, then read back.
    #[arg(long, default_value_t = 4)]
    large_uploads: usize,
    #[arg(long, default_value_t = 256)]
    large_mib: usize,
    /// Blobs read at once.
    #[arg(long, default_value_t = 2_000)]
    reads: usize,
    #[arg(long, default_value_t = 64)]
    read_kib: usize,
    /// HTTP/2 connections the streams spread over.
    #[arg(long, default_value_t = 16)]
    connections: usize,
}

#[derive(clap::Args, Debug)]
struct WorkersArgs {
    /// The worker API, e.g. `grpc://127.0.0.1:50061`.
    #[arg(long)]
    scheduler: String,
    #[arg(long, default_value_t = 10)]
    workers: usize,
    #[arg(long, default_value_t = 1)]
    max_inflight: u64,
    /// Run time per action, uniform in `MIN-MAX` milliseconds.
    #[arg(long, default_value = "500-2000")]
    run_ms: String,
    #[arg(long, default_value_t = 0)]
    fail_percent: u64,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Platform properties each worker advertises, `name=value`.
    #[arg(long = "property")]
    properties: Vec<String>,
    /// Restart one worker, chosen by the seed, this often (0 never).
    #[arg(long, default_value_t = 0)]
    restart_every_s: u64,
    /// Stop after this long (0 runs until killed).
    #[arg(long, default_value_t = 0)]
    duration_s: u64,
    #[arg(long, default_value_t = 10)]
    report_every_s: u64,
}

#[derive(clap::Args, Debug)]
struct QueueArgs {
    /// The CAS endpoint, e.g. `grpc://127.0.0.1:50051`.
    #[arg(long)]
    cas: String,
    /// The execution endpoint; the CAS endpoint when unset.
    #[arg(long)]
    execution: Option<String>,
    #[arg(long, default_value = "main")]
    instance: String,
    #[arg(long, default_value_t = 100)]
    actions: u64,
    /// Executes started per second (0 all at once).
    #[arg(long, default_value_t = 0)]
    rate: u64,
    /// Streams open at once.
    #[arg(long, default_value_t = 10_000)]
    concurrency: usize,
    /// HTTP/2 connections the streams spread over, as Bazel's
    /// `--remote_max_connections` does; one connection carrying thousands
    /// of streams is closed by the server's reset limit.
    #[arg(long, default_value_t = 16)]
    connections: usize,
    /// Distinguishes this run's actions from an earlier run's.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Platform properties each action requires, `name=value`.
    #[arg(long = "property")]
    properties: Vec<String>,
}

fn parse_properties(properties: &[String]) -> Result<HashMap<String, String>, Error> {
    properties
        .iter()
        .map(|p| {
            p.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .ok_or_else(|| make_err!(Code::InvalidArgument, "Property {p} is not name=value"))
        })
        .collect()
}

fn parse_range(range: &str) -> Result<(u64, u64), Error> {
    let (low, high) = range.split_once('-').unwrap_or((range, range));
    let parse = |v: &str| {
        v.parse::<u64>()
            .map_err(|e| make_err!(Code::InvalidArgument, "Bad range {range}: {e}"))
    };
    Ok((parse(low)?, parse(high)?))
}

async fn run_workers(args: WorkersArgs) -> Result<(), Error> {
    let WorkersArgs {
        scheduler,
        workers,
        max_inflight,
        run_ms,
        fail_percent,
        seed,
        properties,
        restart_every_s,
        duration_s,
        report_every_s,
    } = args;
    let run_ms = parse_range(&run_ms)?;
    let properties = parse_properties(&properties)?;
    let counters = Arc::new(Counters::default());
    let source = Arc::new(Distribution {
        seed,
        run_ms,
        fail_percent,
        counters: counters.clone(),
    });
    let fleet: Vec<FleetWorker> = (0..workers)
        .map(|i| FleetWorker {
            name: format!("sim-{i}-"),
            scheduler: scheduler.clone(),
            max_inflight,
            properties: properties.clone(),
        })
        .collect();
    let mut running: Vec<_> = fleet
        .iter()
        .map(|w| spawn_worker(w, source.clone(), counters.clone()))
        .collect();
    eprintln!("fleet-sim: {workers} workers against {scheduler}");
    let lag = spawn_lag_probe();

    let start = Instant::now();
    let mut report = tokio::time::interval(Duration::from_secs(report_every_s.max(1)));
    let mut restart = tokio::time::interval(Duration::from_secs(restart_every_s.max(1)));
    report.tick().await;
    restart.tick().await;
    let mut rng = Rng::new(seed);
    let mut restarts = 0u64;
    loop {
        tokio::select! {
            _ = report.tick() => {
                let alive = running.iter().filter(|w| !w.is_finished()).count();
                let max_lag_ms = lag.swap(0, Ordering::Relaxed);
                eprintln!(
                    "fleet-sim: t={}s alive={alive} restarts={restarts} {counters} max_lag_ms={max_lag_ms}",
                    start.elapsed().as_secs()
                );
            }
            _ = restart.tick(), if restart_every_s > 0 => {
                let i = rng.index(running.len());
                running[i].stop();
                running[i] = spawn_worker(&fleet[i], source.clone(), counters.clone());
                restarts += 1;
            }
        }
        if duration_s > 0 && start.elapsed() >= Duration::from_secs(duration_s) {
            break;
        }
    }
    for worker in &running {
        worker.stop();
    }
    eprintln!("fleet-sim: done restarts={restarts} {counters}");
    Ok(())
}

/// How late, at most, a 100 ms timer has fired since the last read: a
/// simulator starved of CPU shows here, before it shows as a slow
/// scheduler (RFC-0061 §6).
fn spawn_lag_probe() -> Arc<AtomicU64> {
    const TICK: Duration = Duration::from_millis(100);
    let max_lag_ms = Arc::new(AtomicU64::new(0));
    let probe = max_lag_ms.clone();
    drop(background_spawn!("fleet_sim_lag_probe", async move {
        loop {
            let due = Instant::now() + TICK;
            tokio::time::sleep_until(due).await;
            let late =
                u64::try_from(Instant::now().duration_since(due).as_millis()).unwrap_or(u64::MAX);
            probe.fetch_max(late, Ordering::Relaxed);
        }
    }));
    max_lag_ms
}

async fn run_cas(args: CasArgs) -> Result<(), Error> {
    let mut channels = Vec::with_capacity(args.connections.max(1));
    for _ in 0..args.connections.max(1) {
        channels.push(connect(&args.cas).await?);
    }
    let load = CasLoad {
        instance: args.instance,
        seed: args.seed,
        find_missing_digests: args.find_missing_digests,
        large_uploads: args.large_uploads,
        large_mib: args.large_mib,
        reads: args.reads,
        read_kib: args.read_kib,
    };
    let (phases, failures) = cas_load::run(&load, &channels).await?;
    for phase in &phases {
        eprintln!("fleet-sim: {phase}");
    }
    for failure in failures.iter().take(5) {
        eprintln!("fleet-sim: error: {failure}");
    }
    if phases.iter().any(|p| p.errors > 0) {
        return Err(make_err!(
            Code::Internal,
            "the CAS gave wrong or failed answers"
        ));
    }
    Ok(())
}

fn digest_of(bytes: &[u8]) -> Digest {
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(bytes);
    hasher.finalize_digest().into()
}

async fn connect(uri: &str) -> Result<Channel, Error> {
    nativelink_util::tls_utils::endpoint_from(uri, None)?
        .connect()
        .await
        .map_err(|e| make_err!(Code::Unavailable, "Could not connect to {uri}: {e}"))
}

#[derive(Debug, Default)]
struct QueueCounters {
    done: AtomicU64,
    exit_zero: AtomicU64,
    exit_nonzero: AtomicU64,
    failed: AtomicU64,
    reconnects: AtomicU64,
}

/// One client: execute, read to the end, re-subscribe on a dropped stream.
async fn execute_one(
    mut execution: ExecutionClient<Channel>,
    request: ExecuteRequest,
    counters: Arc<QueueCounters>,
) -> Duration {
    let start = Instant::now();
    let mut name: Option<String> = None;
    for _ in 0..10 {
        let response = match &name {
            None => execution.execute(request.clone()).await,
            Some(name) => {
                execution
                    .wait_execution(WaitExecutionRequest { name: name.clone() })
                    .await
            }
        };
        let mut stream = match response {
            Ok(response) => response.into_inner(),
            Err(status) if status.code() == Code::NotFound && name.is_some() => {
                name = None;
                continue;
            }
            Err(_) => {
                counters.reconnects.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        while let Some(item) = stream.next().await {
            let Ok(operation) = item else { break };
            name = Some(operation.name.clone());
            if !operation.done {
                continue;
            }
            counters.done.fetch_add(1, Ordering::Relaxed);
            let exit = match operation.result {
                Some(operation::Result::Response(any)) => {
                    ExecuteResponse::decode(any.value.as_slice())
                        .ok()
                        .filter(|r| r.status.as_ref().is_none_or(|s| s.code == 0))
                        .map(|r| r.result.map_or(0, |a| a.exit_code))
                }
                _ => None,
            };
            match exit {
                Some(0) => counters.exit_zero.fetch_add(1, Ordering::Relaxed),
                Some(_) => counters.exit_nonzero.fetch_add(1, Ordering::Relaxed),
                None => counters.failed.fetch_add(1, Ordering::Relaxed),
            };
            return start.elapsed();
        }
        counters.reconnects.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    counters.done.fetch_add(1, Ordering::Relaxed);
    counters.failed.fetch_add(1, Ordering::Relaxed);
    start.elapsed()
}

async fn run_queue(args: QueueArgs) -> Result<(), Error> {
    let QueueArgs {
        cas,
        execution,
        instance,
        actions,
        rate,
        concurrency,
        connections,
        seed,
        properties,
    } = args;
    let properties = parse_properties(&properties)?;
    let cas_channel = connect(&cas).await?;
    let execution_uri = execution.as_deref().unwrap_or(&cas);
    let mut execution_clients = Vec::with_capacity(connections.max(1));
    for _ in 0..connections.max(1) {
        execution_clients.push(ExecutionClient::new(connect(execution_uri).await?));
    }
    let mut cas_client = ContentAddressableStorageClient::new(cas_channel);

    // Every action shares the empty input root; each has its own command.
    let input_root = Directory::default().encode_to_vec();
    let input_root_digest = digest_of(&input_root);
    let mut blobs = vec![(input_root_digest.clone(), input_root)];
    let mut digests = Vec::new();
    let platform = (!properties.is_empty()).then(|| {
        nativelink_proto::build::bazel::remote::execution::v2::Platform {
            properties: properties
                .iter()
                .map(|(name, value)| {
                    nativelink_proto::build::bazel::remote::execution::v2::platform::Property {
                        name: name.clone(),
                        value: value.clone(),
                    }
                })
                .collect(),
        }
    });
    for i in 0..actions {
        let command = Command {
            arguments: vec!["fleet-sim".into(), format!("{seed}-{i}")],
            ..Default::default()
        }
        .encode_to_vec();
        let command_digest = digest_of(&command);
        let action = Action {
            command_digest: Some(command_digest.clone()),
            input_root_digest: Some(input_root_digest.clone()),
            platform: platform.clone(),
            ..Default::default()
        }
        .encode_to_vec();
        let action_digest = digest_of(&action);
        blobs.push((command_digest, command));
        blobs.push((action_digest.clone(), action));
        digests.push(action_digest);
    }
    for chunk in blobs.chunks(500) {
        let response = cas_client
            .batch_update_blobs(BatchUpdateBlobsRequest {
                instance_name: instance.clone(),
                requests: chunk
                    .iter()
                    .map(|(digest, data)| BlobRequest {
                        digest: Some(digest.clone()),
                        data: Bytes::from(data.clone()),
                        compressor: 0,
                    })
                    .collect(),
                digest_function: digest_function::Value::Sha256.into(),
            })
            .await
            .map_err(|e| make_err!(Code::Internal, "Upload failed: {e}"))?
            .into_inner();
        // The RPC can succeed while a blob is refused in its own status;
        // an action whose blob was refused fails at execute with a missing
        // blob that looks like the scheduler's.
        if let Some(bad) = response
            .responses
            .iter()
            .find(|r| r.status.as_ref().is_some_and(|s| s.code != 0))
        {
            return Err(make_err!(Code::Internal, "Upload refused a blob: {bad:?}"));
        }
    }
    eprintln!("fleet-sim: uploaded {actions} actions to {cas}");

    let counters = Arc::new(QueueCounters::default());
    let permits = Arc::new(Semaphore::new(concurrency.max(1)));
    let start = Instant::now();
    let mut tasks = Vec::with_capacity(digests.len());
    for (i, digest) in digests.into_iter().enumerate() {
        if rate > 0 {
            let due = Duration::from_secs_f64(i as f64 / rate as f64);
            tokio::time::sleep_until(start + due).await;
        }
        let permit = permits
            .clone()
            .acquire_owned()
            .await
            .err_tip(|| "Semaphore closed")?;
        let request = ExecuteRequest {
            instance_name: instance.clone(),
            skip_cache_lookup: true,
            action_digest: Some(digest),
            execution_policy: None,
            results_cache_policy: None,
            digest_function: digest_function::Value::Sha256.into(),
            inline_stdout: false,
            inline_stderr: false,
            inline_output_files: Vec::new(),
        };
        let client = execution_clients[i % execution_clients.len()].clone();
        let counters = counters.clone();
        tasks.push(background_spawn!("fleet_sim_client", async move {
            let took = execute_one(client, request, counters).await;
            drop(permit);
            took
        }));
    }
    let mut report = tokio::time::interval(Duration::from_secs(5));
    report.tick().await;
    let waiting = futures::future::join_all(tasks);
    tokio::pin!(waiting);
    let latencies = loop {
        tokio::select! {
            done = &mut waiting => break done,
            _ = report.tick() => eprintln!(
                "fleet-sim: t={}s done={} of {actions}",
                start.elapsed().as_secs(),
                counters.done.load(Ordering::Relaxed)
            ),
        }
    };
    let mut latencies: Vec<Duration> = latencies.into_iter().filter_map(Result::ok).collect();
    latencies.sort();
    let at = |percent: usize| {
        latencies
            .get(latencies.len().saturating_sub(1) * percent / 100)
            .map_or(0, Duration::as_millis)
    };
    eprintln!(
        "fleet-sim: done in {}s: exit0={} exit_nonzero={} failed={} reconnects={} p50={}ms p95={}ms max={}ms",
        start.elapsed().as_secs(),
        counters.exit_zero.load(Ordering::Relaxed),
        counters.exit_nonzero.load(Ordering::Relaxed),
        counters.failed.load(Ordering::Relaxed),
        counters.reconnects.load(Ordering::Relaxed),
        at(50),
        at(95),
        at(100),
    );
    if counters.failed.load(Ordering::Relaxed) > 0 {
        return Err(make_err!(Code::Internal, "some operations failed"));
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let args = Args::parse();
    #[expect(clippy::disallowed_methods, reason = "the binary's runtime")]
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    #[expect(clippy::disallowed_methods, reason = "the binary's main future")]
    runtime.block_on(async move {
        match args.command {
            Command_::Workers(args) => run_workers(args).await,
            Command_::Queue(args) => run_queue(args).await,
            Command_::Cas(args) => run_cas(args).await,
        }
    })?;
    Ok(())
}
