// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::hash::BuildHasher;
use core::pin::Pin;
use core::str;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use core::time::Duration;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::env;
use std::process::Stdio;
use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::{Future, FutureExt, StreamExt, TryFutureExt, select};
use nativelink_config::cas_server::{EnvironmentSource, LocalWorkerConfig};
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_worker::Update;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::worker_api_client::WorkerApiClient;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    BlobDigestInfo, BlobsAvailableNotification, ExecuteComplete, ExecuteResult, GoingAwayRequest,
    KeepAliveRequest, UpdateForWorker, execute_result,
};
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::FilesystemStore;
use nativelink_util::action_messages::{ActionResult, ActionStage, OperationId};
use nativelink_util::common::{DigestInfo, fs};
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::metrics_utils::{AsyncCounterWrapper, CounterWithTime};
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::store_trait::{ItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo};
use nativelink_util::task::JoinHandleDropGuard;
use nativelink_util::{spawn, tls_utils};
use opentelemetry::context::Context;
use parking_lot::Mutex;
use tokio::process;
use tokio::sync::{Notify, Semaphore, broadcast, mpsc};
use tokio::time::sleep;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::Streaming;
use tracing::{Level, debug, error, event, info, info_span, instrument, trace, warn};

use crate::running_actions_manager::{
    ExecutionConfiguration, Metrics as RunningActionManagerMetrics, RunningAction,
    RunningActionsManager, RunningActionsManagerArgs, RunningActionsManagerImpl,
};
use crate::worker_api_client_wrapper::{WorkerApiClientTrait, WorkerApiClientWrapper};
use crate::worker_utils::make_connect_worker_request;

/// Maximum backstop interval for BlobsAvailable reports (milliseconds).
/// The send loop normally wakes immediately on blob changes via `Notify`,
/// but this backstop ensures subtree-only changes (which don't fire the
/// tracker notify) are still reported within a bounded time.
/// At 100ms with 10 workers the server sees ~100 msgs/s worst case, each
/// coalesced via drain-then-fire. Empty ticks are skipped (no send when
/// there are no changes), so idle workers generate zero traffic.
const BLOBS_AVAILABLE_MAX_INTERVAL_MS: u64 = 100;

/// Platform-specific cumulative CPU time reading.
#[cfg(target_os = "linux")]
mod cpu_impl {
    pub(super) struct CpuTimes {
        pub(super) busy: u64,
        pub(super) total: u64,
    }

    pub(super) fn read_cpu_times() -> Option<CpuTimes> {
        let contents = std::fs::read_to_string("/proc/stat").ok()?;
        let line = contents.lines().next()?;
        if !line.starts_with("cpu ") {
            return None;
        }
        // fields: user(0) nice(1) system(2) idle(3) iowait(4) irq(5) softirq(6) steal(7)
        let fields: Vec<u64> = line[4..]
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if fields.len() < 8 {
            return None;
        }
        let busy = fields[0] + fields[1] + fields[2] + fields[5] + fields[6] + fields[7];
        let total = busy + fields[3] + fields[4];
        Some(CpuTimes { busy, total })
    }
}

#[cfg(target_os = "macos")]
mod cpu_impl {
    const CPU_STATE_USER: usize = 0;
    const CPU_STATE_SYSTEM: usize = 1;
    const CPU_STATE_IDLE: usize = 2;
    const CPU_STATE_NICE: usize = 3;
    const CPU_STATE_MAX: usize = 4;
    const PROCESSOR_CPU_LOAD_INFO: i32 = 2;

    unsafe extern "C" {
        fn mach_host_self() -> u32;
        fn mach_task_self() -> u32;
        fn host_processor_info(
            host: u32,
            flavor: i32,
            out_processor_count: *mut u32,
            out_processor_info: *mut *mut i32,
            out_processor_info_cnt: *mut u32,
        ) -> i32;
        fn vm_deallocate(target_task: u32, address: usize, size: usize) -> i32;
    }

    pub(super) struct CpuTimes {
        pub(super) busy: u64,
        pub(super) total: u64,
    }

    pub(super) struct PerTypeCpuTimes {
        pub(super) aggregate: CpuTimes,
        pub(super) p_core: CpuTimes,
        pub(super) e_core: CpuTimes,
        pub(super) has_e_cores: bool,
    }

    /// Returns the number of P-cores on Apple Silicon via sysctl.
    /// Returns 0 on Intel Macs (sysctl key doesn't exist).
    fn p_core_count() -> u32 {
        use std::sync::OnceLock;
        static COUNT: OnceLock<u32> = OnceLock::new();
        *COUNT.get_or_init(|| sysctl_u32("hw.perflevel0.logicalcpu").unwrap_or(0))
    }

    /// Returns the number of E-cores on Apple Silicon via sysctl.
    /// Returns 0 on Intel Macs or P-core-only Apple Silicon.
    fn e_core_count() -> u32 {
        use std::sync::OnceLock;
        static COUNT: OnceLock<u32> = OnceLock::new();
        *COUNT.get_or_init(|| sysctl_u32("hw.perflevel1.logicalcpu").unwrap_or(0))
    }

    fn sysctl_u32(name: &str) -> Option<u32> {
        use std::ffi::CString;
        let cname = CString::new(name).ok()?;
        let mut val: u32 = 0;
        let mut len = core::mem::size_of::<u32>();
        // SAFETY: sysctlbyname is a stable POSIX API on macOS.
        let ret = unsafe {
            libc::sysctlbyname(
                cname.as_ptr(),
                &raw mut val as *mut _,
                &mut len,
                core::ptr::null_mut(),
                0,
            )
        };
        if ret == 0 { Some(val) } else { None }
    }

    /// Reads per-logical-CPU tick data via host_processor_info and splits
    /// into aggregate, P-core, and E-core buckets.
    pub(super) fn read_per_type_cpu_times() -> Option<PerTypeCpuTimes> {
        use std::sync::OnceLock;
        static HOST_PORT: OnceLock<u32> = OnceLock::new();

        let p_count = p_core_count();
        let e_count = e_core_count();

        // SAFETY: host_processor_info is a stable macOS kernel API.
        // We check the return code and deallocate the kernel-allocated buffer.
        unsafe {
            let host = *HOST_PORT.get_or_init(|| mach_host_self());
            let mut cpu_count: u32 = 0;
            let mut info_array: *mut i32 = core::ptr::null_mut();
            let mut info_count: u32 = 0;
            let ret = host_processor_info(
                host,
                PROCESSOR_CPU_LOAD_INFO,
                &mut cpu_count,
                &mut info_array,
                &mut info_count,
            );
            if ret != 0 || info_array.is_null() {
                return None;
            }

            // On Intel Macs, perflevel sysctl doesn't exist → p_count == 0.
            // Also guard against future chips where the counts don't add up
            // (e.g. a third core type) — fall back to treating all as P-cores.
            let is_heterogeneous = p_count > 0 && (p_count + e_count == cpu_count);

            let mut agg_busy = 0u64;
            let mut agg_total = 0u64;
            let mut p_busy = 0u64;
            let mut p_total = 0u64;
            let mut e_busy = 0u64;
            let mut e_total = 0u64;

            for i in 0..cpu_count {
                let base = (i as usize) * CPU_STATE_MAX;
                let user = *info_array.add(base + CPU_STATE_USER) as u64;
                let system = *info_array.add(base + CPU_STATE_SYSTEM) as u64;
                let idle = *info_array.add(base + CPU_STATE_IDLE) as u64;
                let nice = *info_array.add(base + CPU_STATE_NICE) as u64;
                let busy = user + system + nice;
                let total = busy + idle;
                agg_busy += busy;
                agg_total += total;
                if is_heterogeneous && i < p_count {
                    p_busy += busy;
                    p_total += total;
                } else if is_heterogeneous {
                    e_busy += busy;
                    e_total += total;
                }
            }

            // If not heterogeneous, all cores are P-cores.
            if !is_heterogeneous {
                p_busy = agg_busy;
                p_total = agg_total;
            }

            let kr = vm_deallocate(
                mach_task_self(),
                info_array as usize,
                (info_count as usize) * core::mem::size_of::<i32>(),
            );
            debug_assert_eq!(kr, 0, "vm_deallocate failed: {kr}");

            Some(PerTypeCpuTimes {
                aggregate: CpuTimes { busy: agg_busy, total: agg_total },
                p_core: CpuTimes { busy: p_busy, total: p_total },
                e_core: CpuTimes { busy: e_busy, total: e_total },
                has_e_cores: e_count > 0,
            })
        }
    }

    pub(super) fn read_cpu_times() -> Option<CpuTimes> {
        read_per_type_cpu_times().map(|t| t.aggregate)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod cpu_impl {
    pub(super) struct CpuTimes {
        pub(super) busy: u64,
        pub(super) total: u64,
    }

    pub(super) fn read_cpu_times() -> Option<CpuTimes> {
        None
    }
}

static CPU_PCT: AtomicU32 = AtomicU32::new(0);
static P_CORE_PCT: AtomicU32 = AtomicU32::new(0);
static E_CORE_PCT: AtomicU32 = AtomicU32::new(0);
static SAMPLER_STARTED: AtomicBool = AtomicBool::new(false);

/// Starts a dedicated OS thread that samples system-wide CPU utilization
/// every 100ms. Idempotent — only the first call spawns the thread.
fn start_cpu_sampler() -> Result<(), Error> {
    if SAMPLER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        return Ok(());
    }
    std::thread::Builder::new()
        .name("cpu-sampler".into())
        .spawn(cpu_sample_loop)
        .map_err(|e| make_err!(Code::Internal, "failed to spawn cpu-sampler thread: {:?}", e))?;
    Ok(())
}

fn compute_pct(prev: &cpu_impl::CpuTimes, curr: &cpu_impl::CpuTimes) -> u32 {
    let total_delta = curr.total.wrapping_sub(prev.total);
    let busy_delta = curr.busy.wrapping_sub(prev.busy);
    if total_delta > 0 {
        ((busy_delta as f64 / total_delta as f64) * 100.0).round() as u32
    } else {
        0
    }
}

fn cpu_sample_loop() {
    // Monitoring thread — downgrade to UTILITY QoS so it doesn't
    // compete with real work for P-cores.
    #[cfg(target_os = "macos")]
    {
        const QOS_CLASS_UTILITY: u32 = 0x11;
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        unsafe { pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0) };
    }

    // Try per-type sampling first (macOS with host_processor_info).
    #[cfg(target_os = "macos")]
    {
        if let Some(initial) = cpu_impl::read_per_type_cpu_times() {
            per_type_sample_loop(initial);
            return; // unreachable — loop is infinite
        }
    }

    // Fallback: aggregate-only sampling (Linux, non-macOS, or Intel Mac
    // where host_processor_info failed).
    let mut prev = cpu_impl::read_cpu_times();
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let curr = cpu_impl::read_cpu_times();
        match (&prev, &curr) {
            (Some(p), Some(c)) => {
                CPU_PCT.store(compute_pct(p, c).min(100), Ordering::Relaxed);
            }
            _ => CPU_PCT.store(0, Ordering::Relaxed),
        }
        prev = curr;
    }
}

#[cfg(target_os = "macos")]
fn per_type_sample_loop(initial: cpu_impl::PerTypeCpuTimes) {
    let mut prev = initial;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let Some(curr) = cpu_impl::read_per_type_cpu_times() else {
            CPU_PCT.store(0, Ordering::Relaxed);
            P_CORE_PCT.store(0, Ordering::Relaxed);
            E_CORE_PCT.store(0, Ordering::Relaxed);
            continue;
        };
        CPU_PCT.store(compute_pct(&prev.aggregate, &curr.aggregate).min(100), Ordering::Relaxed);
        P_CORE_PCT.store(compute_pct(&prev.p_core, &curr.p_core).min(100), Ordering::Relaxed);
        if curr.has_e_cores {
            E_CORE_PCT.store(compute_pct(&prev.e_core, &curr.e_core).min(100), Ordering::Relaxed);
        } else {
            // No E-cores → report as fully saturated so scheduler
            // doesn't think idle E-cores are available.
            E_CORE_PCT.store(100, Ordering::Relaxed);
        }
        prev = curr;
    }
}

/// Returns the current system-wide CPU utilization as a percentage (0-100),
/// sampled every 100ms by a dedicated OS thread.
fn get_cpu_load_pct() -> u32 {
    CPU_PCT.load(Ordering::Relaxed)
}

/// Returns the P-core CPU utilization (0-100). 0 means unknown (Linux or
/// non-heterogeneous CPU where per-core-type data is unavailable).
fn get_p_core_load_pct() -> u32 {
    P_CORE_PCT.load(Ordering::Relaxed)
}

/// Returns the E-core CPU utilization (0-100). 0 means unknown.
/// 100 on CPUs without E-cores (all cores are P-cores).
fn get_e_core_load_pct() -> u32 {
    E_CORE_PCT.load(Ordering::Relaxed)
}


/// Build the advertised gRPC endpoint for peer blob sharing.
/// Uses the machine's hostname so a single config works across all workers.
/// The hostname is resolved once and cached for the lifetime of the process.
fn cas_advertised_endpoint(port: u16) -> String {
    use std::sync::OnceLock;
    static HOSTNAME: OnceLock<String> = OnceLock::new();
    let hostname = HOSTNAME.get_or_init(|| {
        match hostname::get() {
            Ok(h) => {
                let name = h.to_string_lossy().into_owned();
                // Append .local for mDNS resolution if the hostname is bare
                // (no dots), so the server can resolve it via multicast DNS.
                if name.contains('.') {
                    name
                } else {
                    format!("{name}.local")
                }
            }
            Err(err) => {
                error!(
                    ?err,
                    "hostname::get() failed, using 'localhost' — peer blob sharing will not work across machines"
                );
                "localhost".to_string()
            }
        }
    });
    format!("grpc://{hostname}:{port}")
}

/// Start a QUIC/H3 server for the worker CAS, alongside the TCP server.
///
/// Generates a self-signed TLS certificate at startup (QUIC mandates TLS 1.3)
/// and binds a UDP socket on the same port as the TCP server. Peer workers
/// connecting with `use_http3: true` will use this QUIC endpoint for blob
/// fetches, benefiting from QUIC's built-in stream multiplexing.
#[cfg(feature = "quic")]
fn start_worker_quic_server(
    port: u16,
    worker_name: &str,
    routes: tonic::service::Routes,
) -> Result<JoinHandleDropGuard<Result<(), Error>>, Error> {
    use std::sync::Arc;
    use h3_quinn as _;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

    // Generate self-signed certificate for this worker.
    let cert = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        worker_name.to_string(),
    ])
    .map_err(|e| make_err!(Code::Internal, "Failed to generate self-signed cert: {e:?}"))?;

    let cert_der = CertificateDer::from(cert.cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        cert.signing_key.serialize_der(),
    ));

    let mut tls_config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .map_err(|e| make_err!(Code::Internal, "Worker QUIC TLS version error: {e:?}"))?
    .with_no_client_auth()
    .with_single_cert(vec![cert_der], key_der)
    .map_err(|e| make_err!(Code::Internal, "Worker QUIC TLS config error: {e:?}"))?;
    tls_config.alpn_protocols = vec![b"h3".to_vec()];
    tls_config.max_early_data_size = u32::MAX;

    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(tls_config))
            .map_err(|e| make_err!(Code::Internal, "Worker Quinn server config error: {e:?}"))?,
    ));

    // Tune QUIC transport for LAN usage.
    let mut transport = quinn::TransportConfig::default();
    transport.stream_receive_window((16 * 1024 * 1024u32).into());
    transport.receive_window((128 * 1024 * 1024u32).into());
    transport.send_window(128 * 1024 * 1024);
    transport.max_concurrent_bidi_streams(1024u32.into());
    transport.max_concurrent_uni_streams(1024u32.into());
    transport.initial_rtt(Duration::from_micros(500));
    // Match server/client idle timeout for consistent behavior.
    transport.max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()));
    // Send QUIC keepalives every 5s to detect dead connections and
    // prevent NAT/firewall timeouts on the server→worker path.
    transport.keep_alive_interval(Some(Duration::from_secs(5)));
    server_config.transport_config(Arc::new(transport));

    // Bind UDP socket with large buffers.
    let socket_addr: std::net::SocketAddr = ([0, 0, 0, 0], port).into();
    let udp_socket = std::net::UdpSocket::bind(socket_addr)
        .map_err(|e| make_err!(Code::Internal, "Worker QUIC UDP bind on {socket_addr}: {e:?}"))?;
    {
        const QUIC_UDP_BUF: usize = 8 * 1024 * 1024;
        let sock_ref = socket2::SockRef::from(&udp_socket);
        if let Err(err) = sock_ref.set_send_buffer_size(QUIC_UDP_BUF) {
            info!(?err, "Failed to set worker QUIC SO_SNDBUF");
        }
        if let Err(err) = sock_ref.set_recv_buffer_size(QUIC_UDP_BUF) {
            info!(?err, "Failed to set worker QUIC SO_RCVBUF");
        }
    }

    let quinn_endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(server_config),
        udp_socket,
        quinn::default_runtime()
            .ok_or_else(|| make_err!(Code::Internal, "No async runtime for worker QUIC"))?,
    )
    .map_err(|e| make_err!(Code::Internal, "Failed to create worker QUIC endpoint: {e:?}"))?;

    let acceptor = tonic_h3::quinn::H3QuinnAcceptor::new(quinn_endpoint);
    let h3_router = tonic_h3::server::H3Router::new(routes);

    let worker_name = worker_name.to_string();
    info!(
        worker_name = %worker_name,
        %socket_addr,
        "Starting worker CAS QUIC/H3 server for peer blob sharing"
    );

    Ok(spawn!("worker_cas_quic", async move {
        if let Err(err) = h3_router.serve(acceptor).await {
            error!(?err, "Worker CAS QUIC/H3 server error");
            return Err(make_err!(Code::Internal, "Worker CAS QUIC server: {err:?}"));
        }
        Ok(())
    }))
}

/// Accumulated blob changes between BlobsAvailable ticks.
#[derive(Debug, Default)]
pub struct BlobChanges {
    /// digest → last_access_timestamp (unix seconds).
    pub added: HashMap<DigestInfo, i64>,
    pub evicted: HashSet<DigestInfo>,
}

/// Tracks inserts and evictions from the FilesystemStore between ticks.
/// Registered as a callback on the FilesystemStore's evicting map.
///
/// Contains a `Notify` that is signalled on every insert or eviction so
/// the BlobsAvailable send loop can wake immediately instead of polling
/// on a fixed interval.
#[derive(Debug)]
pub struct BlobChangeTracker {
    pending: Mutex<BlobChanges>,
    /// Wakes the BlobsAvailable send loop when changes accumulate.
    notify: Arc<Notify>,
}

impl BlobChangeTracker {
    pub fn new(notify: Arc<Notify>) -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(BlobChanges::default()),
            notify,
        })
    }

    /// Atomically swap out accumulated changes, returning them.
    /// The internal state is replaced with an empty BlobChanges.
    pub fn swap(&self) -> BlobChanges {
        let mut pending = self.pending.lock();
        std::mem::take(&mut *pending)
    }
}

impl ItemCallback for BlobChangeTracker {
    // On evict: add to evicted, remove from added (cancel out insert+evict).
    fn callback<'a>(
        &'a self,
        store_key: StoreKey<'a>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        if let StoreKey::Digest(digest) = store_key {
            let mut pending = self.pending.lock();
            pending.added.remove(&digest);
            pending.evicted.insert(digest);
            self.notify.notify_one();
        }
        Box::pin(core::future::ready(()))
    }

    // On insert: add to added, remove from evicted (cancel out evict+reinsert).
    fn on_insert(&self, store_key: StoreKey<'_>, _size: u64) {
        if let StoreKey::Digest(digest) = store_key {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let mut pending = self.pending.lock();
            pending.evicted.remove(&digest);
            pending.added.insert(digest, ts);
            self.notify.notify_one();
        }
    }
}

/// Amount of time to wait if we have actions in transit before we try to
/// consider an error to have occurred.
const ACTIONS_IN_TRANSIT_TIMEOUT_S: f32 = 10.;

/// If we lose connection to the worker api server we will wait this many seconds
/// before trying to connect.
const CONNECTION_RETRY_DELAY_S: f32 = 0.5;

/// Default endpoint timeout. If this value gets modified the documentation in
/// `cas_server.rs` must also be updated.
const DEFAULT_ENDPOINT_TIMEOUT_S: f32 = 5.;

/// Default maximum amount of time a task is allowed to run for.
/// If this value gets modified the documentation in `cas_server.rs` must also be updated.
const DEFAULT_MAX_ACTION_TIMEOUT: Duration = Duration::from_secs(1200); // 20 mins.
const DEFAULT_MAX_UPLOAD_TIMEOUT: Duration = Duration::from_secs(600); // 10 mins.

/// Holds the FilesystemStore reference and change tracker needed for
/// BlobsAvailable reporting with drain-then-fire semantics.
#[derive(Clone, Debug)]
pub struct BlobsAvailableState {
    /// Reference to the worker's local FilesystemStore (the fast store in FastSlowStore).
    fs_store: Arc<FilesystemStore>,
    /// Tracks inserted and evicted digests between sends.
    tracker: Arc<BlobChangeTracker>,
    /// The worker's CAS endpoint for peer serving (e.g. "grpc://192.168.191.5:50081").
    cas_endpoint: String,
    /// Woken by the tracker on every insert/eviction so the send loop fires
    /// immediately instead of sleeping for a fixed interval.
    notify: Arc<Notify>,
    /// Backstop interval: even without blob changes, wake periodically to
    /// pick up subtree-only deltas that bypass the tracker notify.
    max_interval: Duration,
}

struct LocalWorkerImpl<'a, T: WorkerApiClientTrait + 'static, U: RunningActionsManager> {
    config: &'a LocalWorkerConfig,
    // According to the tonic documentation it is a cheap operation to clone this.
    grpc_client: T,
    worker_id: String,
    running_actions_manager: Arc<U>,
    // Number of actions that have been received in `Update::StartAction`, but
    // not yet processed by running_actions_manager's spawn. This number should
    // always be zero if there are no actions running and no actions being waited
    // on by the scheduler.
    actions_in_transit: Arc<AtomicU64>,
    metrics: Arc<Metrics>,
    /// State for periodic BlobsAvailable reporting. None if disabled (no CAS endpoint).
    blobs_available_state: Option<BlobsAvailableState>,
}

pub async fn preconditions_met<H: BuildHasher + Sync>(
    precondition_script: Option<String>,
    extra_envs: &HashMap<String, String, H>,
) -> Result<(), Error> {
    let Some(precondition_script) = &precondition_script else {
        // No script means we are always ok to proceed.
        return Ok(());
    };
    // TODO: Might want to pass some information about the command to the
    //       script, but at this point it's not even been downloaded yet,
    //       so that's not currently possible.  Perhaps we'll move this in
    //       future to pass useful information through?  Or perhaps we'll
    //       have a pre-condition and a pre-execute script instead, although
    //       arguably entrypoint already gives us that.

    let maybe_split_cmd = shlex::split(precondition_script);
    let (command, args) = match &maybe_split_cmd {
        Some(split_cmd) => (&split_cmd[0], &split_cmd[1..]),
        None => {
            return Err(make_input_err!(
                "Could not parse the value of precondition_script: '{}'",
                precondition_script,
            ));
        }
    };

    let precondition_process = process::Command::new(command)
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_clear()
        .envs(extra_envs)
        .spawn()
        .err_tip(|| format!("Could not execute precondition command {precondition_script:?}"))?;
    let output = precondition_process.wait_with_output().await?;
    let stdout = str::from_utf8(&output.stdout).unwrap_or("");
    trace!(status = %output.status, %stdout, "Preconditions script returned");
    if output.status.code() == Some(0) {
        Ok(())
    } else {
        Err(make_err!(
            Code::ResourceExhausted,
            "Preconditions script returned status {} - {}",
            output.status,
            stdout
        ))
    }
}

impl<'a, T: WorkerApiClientTrait + 'static, U: RunningActionsManager> LocalWorkerImpl<'a, T, U> {
    fn new(
        config: &'a LocalWorkerConfig,
        grpc_client: T,
        worker_id: String,
        running_actions_manager: Arc<U>,
        metrics: Arc<Metrics>,
        blobs_available_state: Option<BlobsAvailableState>,
    ) -> Self {
        Self {
            config,
            grpc_client,
            worker_id,
            running_actions_manager,
            // Number of actions that have been received in `Update::StartAction`, but
            // not yet processed by running_actions_manager's spawn. This number should
            // always be zero if there are no actions running and no actions being waited
            // on by the scheduler.
            actions_in_transit: Arc::new(AtomicU64::new(0)),
            metrics,
            blobs_available_state,
        }
    }

    /// Upload blobs requested by the server's UploadMissingBlobs message.
    /// Reads from the local fast store and writes to the slow store (server CAS).
    async fn handle_upload_missing_blobs(
        running_actions_manager: &Arc<U>,
        digests: Vec<DigestInfo>,
    ) {
        let Some(cas_store) = running_actions_manager.get_cas_store() else {
            warn!("UploadMissingBlobs: no CAS store available, ignoring");
            return;
        };
        let fast_store = cas_store.fast_store();
        let slow_store = cas_store.slow_store();
        if slow_store
            .inner_store(None::<StoreKey<'_>>)
            .optimized_for(nativelink_util::store_trait::StoreOptimizations::NoopUpdates)
        {
            return;
        }

        // Check which blobs we actually have locally before uploading.
        let keys: Vec<StoreKey<'_>> = digests
            .iter()
            .map(|d| StoreKey::from(*d))
            .collect();
        let mut results = vec![None; keys.len()];
        if let Err(err) = fast_store.has_with_results(&keys, &mut results).await {
            warn!(?err, "UploadMissingBlobs: failed to check local store");
            return;
        }

        let present: Vec<DigestInfo> = digests
            .iter()
            .zip(results.iter())
            .filter_map(|(d, r)| if r.is_some() { Some(*d) } else { None })
            .collect();

        if present.is_empty() {
            info!(
                requested = digests.len(),
                "UploadMissingBlobs: none of the requested blobs found locally"
            );
            return;
        }

        info!(
            requested = digests.len(),
            found = present.len(),
            "UploadMissingBlobs: uploading blobs to server"
        );

        const MAX_CONCURRENT_UPLOADS: usize = 32;
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_UPLOADS));

        let mut uploads: FuturesUnordered<_> = present
            .iter()
            .map(|&digest| {
                let fast_store = fast_store.clone();
                let slow_store = slow_store.clone();
                let semaphore = semaphore.clone();
                async move {
                    let _permit = semaphore
                        .acquire()
                        .await
                        .expect("semaphore should not be closed");
                    // Use in-memory transfer for small blobs, streaming for
                    // large ones to avoid OOM on multi-GB blobs.
                    const STREAMING_THRESHOLD: u64 = 1024 * 1024; // 1 MiB
                    let result = if digest.size_bytes() <= STREAMING_THRESHOLD {
                        match fast_store.get_part_unchunked(digest, 0, None).await {
                            Ok(data) => slow_store.update_oneshot(digest, data).await,
                            Err(err) => Err(err),
                        }
                    } else {
                        let (tx, rx) = make_buf_channel_pair();
                        let read_fut = fast_store.get(digest, tx);
                        let write_fut = slow_store.update(
                            digest,
                            rx,
                            UploadSizeInfo::ExactSize(digest.size_bytes()),
                        );
                        let (read_res, write_res) = tokio::join!(read_fut, write_fut);
                        if write_res.is_ok() {
                            Ok(())
                        } else {
                            read_res.merge(write_res)
                        }
                    };
                    match result {
                        Ok(()) => true,
                        Err(err) => {
                            warn!(
                                ?digest,
                                ?err,
                                "UploadMissingBlobs: failed to transfer blob"
                            );
                            false
                        }
                    }
                }
            })
            .collect();

        let mut uploaded = 0usize;
        let mut failed = 0usize;
        while let Some(ok) = uploads.next().await {
            if ok {
                uploaded += 1;
            } else {
                failed += 1;
            }
        }

        info!(
            uploaded,
            failed,
            total = present.len(),
            "UploadMissingBlobs: backfill complete"
        );
    }

    /// Starts a background spawn/thread that will send a message to the server every `timeout / 2`.
    async fn start_keep_alive(&self) -> Result<(), Error> {
        // According to tonic's documentation this call should be cheap and is the same stream.
        let mut grpc_client = self.grpc_client.clone();

        loop {
            let timeout = self
                .config
                .worker_api_endpoint
                .timeout
                .unwrap_or(DEFAULT_ENDPOINT_TIMEOUT_S);
            // We always send 2 keep alive requests per timeout. Http2 should manage most of our
            // timeout issues, this is a secondary check to ensure we can still send data.
            sleep(Duration::from_secs_f32(timeout / 2.)).await;
            let load = get_cpu_load_pct();
            let p_load = get_p_core_load_pct();
            let e_load = get_e_core_load_pct();
            debug!("KeepAlive cpu_load_pct={load} p_core={p_load} e_core={e_load}");
            if let Err(e) = grpc_client.keep_alive(KeepAliveRequest {
                cpu_load_pct: load,
                p_core_load_pct: p_load,
                e_core_load_pct: e_load,
            }).await {
                return Err(make_err!(
                    Code::Internal,
                    "Failed to send KeepAlive in LocalWorker : {:?}",
                    e
                ));
            }
        }
    }

    /// Sends a periodic BlobsAvailable notification.
    /// - First tick: full snapshot of all digests with timestamps (scans store once).
    ///   Also sends a full subtree snapshot with ALL subtree digests.
    /// - Subsequent ticks: delta from callback-accumulated changes (no scan).
    ///   Sends delta-encoded subtree changes (added/removed).
    async fn send_periodic_blobs_available(
        grpc_client: &mut T,
        state: &BlobsAvailableState,
        running_actions_manager: &Arc<U>,
        is_first: bool,
    ) -> Result<(), Error> {
        let (digest_infos, evicted_digests) = if is_first {
            // Full snapshot: scan everything once.
            let all = state.fs_store.get_all_digests_with_timestamps();
            // Drain any changes that accumulated during startup.
            drop(state.tracker.swap());

            let infos: Vec<BlobDigestInfo> = all
                .iter()
                .map(|(digest, ts)| BlobDigestInfo {
                    digest: Some((*digest).into()),
                    last_access_timestamp: *ts,
                })
                .collect();

            (infos, Vec::new())
        } else {
            // Delta: swap out accumulated changes.
            let changes = state.tracker.swap();
            if changes.added.is_empty() && changes.evicted.is_empty() {
                // Even if no blob changes, we may have subtree changes to report.
                // We'll check below and skip only if both are empty.
            }

            let infos: Vec<BlobDigestInfo> = changes
                .added
                .iter()
                .map(|(digest, &ts)| BlobDigestInfo {
                    digest: Some((*digest).into()),
                    last_access_timestamp: ts,
                })
                .collect();
            let evicted_protos = changes.evicted.iter().map(|d| (*d).into()).collect();

            (infos, evicted_protos)
        };

        // Collect subtree delta or full snapshot.
        let (cached_directory_digests, added_subtree_digests, removed_subtree_digests, is_full_subtree_snapshot) = if is_first {
            // Full subtree snapshot: send ALL subtree digests in cached_directory_digests.
            // Also drain any pending changes accumulated during startup.
            drop(running_actions_manager.take_pending_subtree_changes().await);
            let all_subtrees = running_actions_manager.all_subtree_digests().await;
            let all_subtree_protos = all_subtrees.into_iter().map(|d| d.into()).collect();
            (all_subtree_protos, Vec::new(), Vec::new(), true)
        } else {
            // Delta: take pending subtree changes.
            let (added, removed) = running_actions_manager.take_pending_subtree_changes().await;
            let added_protos = added.into_iter().map(|d| d.into()).collect();
            let removed_protos = removed.into_iter().map(|d| d.into()).collect();
            (Vec::new(), added_protos, removed_protos, false)
        };

        let new_or_touched_count = digest_infos.len();
        let evicted_count = evicted_digests.len();
        let cached_dir_count = cached_directory_digests.len();
        let added_subtree_count = added_subtree_digests.len();
        let removed_subtree_count = removed_subtree_digests.len();

        // Skip sending if there are truly no changes at all.
        if !is_first
            && new_or_touched_count == 0
            && evicted_count == 0
            && added_subtree_count == 0
            && removed_subtree_count == 0
        {
            trace!("BlobsAvailable: no changes since last tick, skipping");
            return Ok(());
        }

        let load = get_cpu_load_pct();
        let p_load = get_p_core_load_pct();
        let e_load = get_e_core_load_pct();
        debug!("BlobsAvailable cpu_load_pct={load} p_core={p_load} e_core={e_load}");
        let notification = BlobsAvailableNotification {
            worker_cas_endpoint: state.cas_endpoint.clone(),
            digests: Vec::new(),
            is_full_snapshot: is_first,
            evicted_digests,
            digest_infos,
            cpu_load_pct: load,
            cached_directory_digests,
            added_subtree_digests,
            removed_subtree_digests,
            is_full_subtree_snapshot,
            p_core_load_pct: p_load,
            e_core_load_pct: e_load,
        };

        if let Err(err) = grpc_client.blobs_available(notification).await {
            warn!(
                ?err,
                new_or_touched_count,
                evicted_count,
                cached_dir_count,
                added_subtree_count,
                removed_subtree_count,
                is_first,
                "Failed to send periodic BlobsAvailable"
            );
            // Channel closed means the server dropped us — propagate to
            // trigger reconnect. The server also sends Update::Disconnect
            // when it detects "Worker not found", which is handled in run().
            return Err(err);
        } else {
            info!(
                new_or_touched_count,
                evicted_count,
                cached_dir_count,
                added_subtree_count,
                removed_subtree_count,
                is_first,
                "Sent periodic BlobsAvailable"
            );
        }
        Ok(())
    }

    async fn run(
        &self,
        update_for_worker_stream: Streaming<UpdateForWorker>,
        shutdown_rx: &mut broadcast::Receiver<ShutdownGuard>,
    ) -> Result<(), Error> {
        // This big block of logic is designed to help simplify upstream components. Upstream
        // components can write standard futures that return a `Result<(), Error>` and this block
        // will forward the error up to the client and disconnect from the scheduler.
        // It is a common use case that an item sent through update_for_worker_stream will always
        // have a response but the response will be triggered through a callback to the scheduler.
        // This can be quite tricky to manage, so what we have done here is given access to a
        // `futures` variable which because this is in a single thread as well as a channel that you
        // send a future into that makes it into the `futures` variable.
        // This means that if you want to perform an action based on the result of the future
        // you use the `.map()` method and the new action will always come to live in this spawn,
        // giving mutable access to stuff in this struct.
        // NOTE: If you ever return from this function it will disconnect from the scheduler.
        let mut futures = FuturesUnordered::new();
        futures.push(self.start_keep_alive().boxed());

        // Start BlobsAvailable reporting with drain-then-fire semantics.
        // The loop wakes immediately when blob changes are detected (via
        // Notify) and drains all accumulated changes in one send. Under
        // high load, changes accumulate while the previous send is in
        // flight and are picked up by the next iteration.
        if let Some(ref state) = self.blobs_available_state {
            let mut grpc_client = self.grpc_client.clone();
            let state = state.clone();
            let ram = self.running_actions_manager.clone();
            futures.push(
                async move {
                    // Send full snapshot immediately on connect so the
                    // server has an accurate locality map right away.
                    Self::send_periodic_blobs_available(
                        &mut grpc_client,
                        &state,
                        &ram,
                        true,
                    )
                    .await?;
                    loop {
                        // Wait for either:
                        // 1. A blob insert/eviction notification (immediate wake), or
                        // 2. The backstop interval (catches subtree-only changes).
                        tokio::select! {
                            () = state.notify.notified() => {}
                            () = sleep(state.max_interval) => {}
                        }
                        Self::send_periodic_blobs_available(
                            &mut grpc_client,
                            &state,
                            &ram,
                            false,
                        )
                        .await?;
                    }
                }
                .boxed(),
            );
        }

        let (add_future_channel, add_future_rx) = mpsc::unbounded_channel();
        let mut add_future_rx = UnboundedReceiverStream::new(add_future_rx).fuse();

        let mut update_for_worker_stream = update_for_worker_stream.fuse();
        // A notify which is triggered every time actions_in_flight is subtracted.
        let actions_notify = Arc::new(Notify::new());
        // A counter of actions that are in-flight, this is similar to actions_in_transit but
        // includes the AC upload and notification to the scheduler.
        let actions_in_flight = Arc::new(AtomicU64::new(0));
        // Set to true when shutting down, this stops any new StartAction.
        let mut shutting_down = false;

        loop {
            select! {
                maybe_update = update_for_worker_stream.next() => if !shutting_down || maybe_update.is_some() {
                    match maybe_update
                        .err_tip(|| "UpdateForWorker stream closed early")?
                        .err_tip(|| "Got error in UpdateForWorker stream")?
                        .update
                        .err_tip(|| "Expected update to exist in UpdateForWorker")?
                    {
                        Update::ConnectionResult(_) => {
                            return Err(make_input_err!(
                                "Got ConnectionResult in LocalWorker::run which should never happen"
                            ));
                        }
                        Update::Disconnect(()) => {
                            self.metrics.disconnects_received.inc();
                            return Err(make_err!(
                                Code::Internal,
                                "received disconnect from scheduler, will reconnect"
                            ));
                        }
                        Update::KeepAlive(()) => {
                            self.metrics.keep_alives_received.inc();
                        }
                        Update::KillOperationRequest(kill_operation_request) => {
                            let operation_id = OperationId::from(kill_operation_request.operation_id);
                            if let Err(err) = self.running_actions_manager.kill_operation(&operation_id).await {
                                error!(
                                    %operation_id,
                                    ?err,
                                    "Failed to send kill request for operation"
                                );
                            }
                        }
                        Update::TouchBlobs(touch_request) => {
                            // Touch blobs in the local store to update access times
                            // and prevent premature eviction of referenced blobs.
                            let digest_count = touch_request.digests.len();
                            trace!(digest_count, "Received TouchBlobs request");
                            if let Some(ref state) = self.blobs_available_state {
                                let fs_store = state.fs_store.clone();
                                let digests: Vec<DigestInfo> = touch_request
                                    .digests
                                    .into_iter()
                                    .filter_map(|d| DigestInfo::try_from(d).ok())
                                    .collect();
                                // Best-effort: call has() on each digest to update
                                // the EvictingMap's LRU access time.
                                let keys: Vec<StoreKey<'_>> = digests
                                    .iter()
                                    .map(|d| StoreKey::from(*d))
                                    .collect();
                                let mut results = vec![None; keys.len()];
                                if let Err(err) = Pin::new(fs_store.as_ref())
                                    .has_with_results(&keys, &mut results)
                                    .await
                                {
                                    warn!(
                                        ?err,
                                        digest_count,
                                        "TouchBlobs: failed to touch digests in FilesystemStore"
                                    );
                                } else {
                                    let found = results.iter().filter(|r| r.is_some()).count();
                                    trace!(
                                        digest_count,
                                        found,
                                        "TouchBlobs: touched digests in FilesystemStore"
                                    );
                                }
                            }
                        }
                        Update::BlobsInStableStorage(blobs) => {
                            // Server confirms these blobs are persisted to stable storage.
                            // Unpin them from the local FilesystemStore so they become
                            // eligible for eviction again.
                            let digest_count = blobs.digests.len();
                            if let Some(ref state) = self.blobs_available_state {
                                let fs_store = &state.fs_store;
                                let mut unpinned = 0usize;
                                for proto_digest in &blobs.digests {
                                    if let Ok(digest) = DigestInfo::try_from(proto_digest.clone()) {
                                        fs_store.unpin_digest(&digest);
                                        unpinned += 1;
                                    } else {
                                        warn!(
                                            ?proto_digest,
                                            "BlobsInStableStorage: invalid digest, skipping unpin"
                                        );
                                    }
                                }
                                info!(
                                    unpinned,
                                    digest_count,
                                    "BlobsInStableStorage: unpinned digests from local CAS"
                                );
                            } else {
                                trace!(
                                    digest_count,
                                    "BlobsInStableStorage: no FilesystemStore available, ignoring"
                                );
                            }
                        }
                        Update::UploadMissingBlobs(request) => {
                            // Server is requesting we upload blobs it doesn't
                            // have. Read from local fast store and upload to
                            // the slow store (server CAS) in the background.
                            let digest_count = request.digests.len();
                            let digests: Vec<DigestInfo> = request
                                .digests
                                .into_iter()
                                .filter_map(|d| DigestInfo::try_from(d).ok())
                                .collect();
                            info!(
                                digest_count,
                                valid_count = digests.len(),
                                "UploadMissingBlobs: server requests blob backfill"
                            );
                            let ram = self.running_actions_manager.clone();
                            tokio::spawn(async move {
                                Self::handle_upload_missing_blobs(&ram, digests).await;
                            });
                        }
                        Update::StartAction(start_execute) => {
                            // Don't accept any new requests if we're shutting down.
                            if shutting_down {
                                if let Some(instance_name) = start_execute.execute_request.map(|request| request.instance_name) {
                                    self.grpc_client.clone().execution_response(
                                        ExecuteResult{
                                            instance_name,
                                            operation_id: start_execute.operation_id,
                                            result: Some(execute_result::Result::InternalError(make_err!(Code::ResourceExhausted, "Worker shutting down").into())),
                                        }
                                    ).await?;
                                }
                                continue;
                            }

                            self.metrics.start_actions_received.inc();

                            let execute_request = start_execute.execute_request.as_ref();
                            let operation_id = start_execute.operation_id.clone();
                            let operation_id_to_log = operation_id.clone();
                            let maybe_instance_name = execute_request.map(|v| v.instance_name.clone());
                            let action_digest = execute_request.and_then(|v| v.action_digest.clone());
                            let digest_hasher = execute_request
                                .ok_or_else(|| make_input_err!("Expected execute_request to be set"))
                                .and_then(|v| DigestHasherFunc::try_from(v.digest_function))
                                .err_tip(|| "In LocalWorkerImpl::new()")?;

                            let start_action_fut = {
                                let precondition_script_cfg = self.config.experimental_precondition_script.clone();
                                let mut extra_envs: HashMap<String, String> = HashMap::new();
                                if let Some(ref additional_environment) = self.config.additional_environment {
                                    for (name, source) in additional_environment {
                                        let value = match source {
                                            EnvironmentSource::Property(property) => start_execute
                                                .platform.as_ref().and_then(|p|p.properties.iter().find(|pr| &pr.name == property))
                                                .map_or_else(|| Cow::Borrowed(""), |v| Cow::Borrowed(v.value.as_str())),
                                            EnvironmentSource::Value(value) => Cow::Borrowed(value.as_str()),
                                            EnvironmentSource::FromEnvironment => Cow::Owned(env::var(name).unwrap_or_default()),
                                            other => {
                                                debug!(?other, "Worker doesn't support this type of additional environment");
                                                continue;
                                            }
                                        };
                                        extra_envs.insert(name.clone(), value.into_owned());
                                    }
                                }
                                let actions_in_transit = self.actions_in_transit.clone();
                                let worker_id = self.worker_id.clone();
                                let running_actions_manager = self.running_actions_manager.clone();
                                self.metrics.clone().wrap(move |metrics| async move {
                                    metrics.preconditions.wrap(preconditions_met(precondition_script_cfg, &extra_envs))
                                    .and_then(|()| running_actions_manager.create_and_add_action(worker_id, start_execute))
                                    .map(move |r| {
                                        // Now that we either failed or registered our action, we can
                                        // consider the action to no longer be in transit.
                                        actions_in_transit.fetch_sub(1, Ordering::Release);
                                        r
                                    })
                                    .and_then(|action| {
                                        debug!(
                                            operation_id = %action.get_operation_id(),
                                            "Received request to run action"
                                        );
                                        // Box each phase to heap-allocate its future state
                                        // separately. Without this, the compiler generates a
                                        // single monolithic state machine for the entire
                                        // AndThen chain, which overflows the 8 MiB stack in
                                        // debug builds.
                                        Box::pin(action.clone().prepare_action())
                                            .and_then(|a| Box::pin(RunningAction::execute(a)))
                                            // upload_results now only uploads to the local fast store
                                            // (FilesystemStore). The remote CAS upload is deferred to
                                            // the background after the result is reported.
                                            .and_then(|a| Box::pin(RunningAction::upload_results(a)))
                                            .and_then(|a| Box::pin(RunningAction::get_finished_result(a)))
                                            .then(|result| async move {
                                                // Spawn cleanup in the background — it only removes
                                                // the work directory (files already renamed into CAS).
                                                // The cleaning_up_operations + wait_for_cleanup mechanism
                                                // handles the race if the same action is retried.
                                                tokio::spawn(async move {
                                                    if let Err(e) = action.cleanup().await {
                                                        error!(?e, "Background cleanup failed");
                                                    }
                                                });
                                                result
                                            })
                                    }).await
                                })
                            };

                            let make_publish_future = {
                                let mut grpc_client = self.grpc_client.clone();
                                let cas_endpoint_for_notify = self.config.cas_server_port
                                    .map(|port| cas_advertised_endpoint(port))
                                    .unwrap_or_default();

                                let running_actions_manager = self.running_actions_manager.clone();
                                move |res: Result<ActionResult, Error>| async move {
                                    // Sample CPU at completion time, not action start time.
                                    let exec_load = get_cpu_load_pct();
                                    let exec_p_load = get_p_core_load_pct();
                                    let exec_e_load = get_e_core_load_pct();
                                    debug!("ExecuteComplete cpu_load_pct={exec_load} p_core={exec_p_load} e_core={exec_e_load}");
                                    let complete = ExecuteComplete {
                                        operation_id: operation_id.clone(),
                                        cpu_load_pct: exec_load,
                                        p_core_load_pct: exec_p_load,
                                        e_core_load_pct: exec_e_load,
                                    };
                                    let instance_name = maybe_instance_name
                                        .err_tip(|| "`instance_name` could not be resolved; this is likely an internal error in local_worker.")?;
                                    match res {
                                        Ok(mut action_result) => {
                                            // 1. Send execution response FIRST to minimize
                                            //    critical-path latency for Bazel. The
                                            //    ActionResult is embedded in the
                                            //    ExecuteResponse proto, so Bazel doesn't
                                            //    need the AC entry for the current build.
                                            //    The server's inner_execution_response()
                                            //    also calls register_action_result_digests
                                            //    from the response itself, so blob locality
                                            //    is known even before BlobsAvailable arrives.
                                            let action_stage = ActionStage::Completed(action_result.clone());
                                            grpc_client.execution_response(
                                                ExecuteResult{
                                                    instance_name,
                                                    operation_id,
                                                    result: Some(execute_result::Result::ExecuteResponse(action_stage.into())),
                                                }
                                            )
                                            .await
                                            .err_tip(|| "Error while calling execution_response")?;

                                            // 2. Free the worker for new actions.
                                            drop(grpc_client.execution_complete(complete).await);

                                            // 3. AC write — needs &mut action_result so runs
                                            //    before the tree expansion / BlobsAvailable
                                            //    that borrow it immutably.
                                            if let Some(digest_info) = action_digest.clone().and_then(|action_digest| action_digest.try_into().ok()) {
                                                if let Err(err) = running_actions_manager.cache_action_result(digest_info, &mut action_result, digest_hasher).await {
                                                    error!(
                                                        ?err,
                                                        ?action_digest,
                                                        "Error saving action in store",
                                                    );
                                                }
                                            }

                                            // 4. Tree expansion + BlobsAvailable are off the
                                            //    critical path. Tree expansion reads Tree
                                            //    blobs from local CAS which can be slow, and
                                            //    is only needed for the locality map
                                            //    notification, not the ExecuteResponse.
                                            if !cas_endpoint_for_notify.is_empty() {
                                                let mut output_digests = Vec::new();
                                                for file in &action_result.output_files {
                                                    output_digests.push(file.digest.into());
                                                }
                                                for folder in &action_result.output_folders {
                                                    output_digests.push(folder.tree_digest.into());
                                                }
                                                if action_result.stdout_digest.size_bytes() > 0 {
                                                    output_digests.push(action_result.stdout_digest.into());
                                                }
                                                if action_result.stderr_digest.size_bytes() > 0 {
                                                    output_digests.push(action_result.stderr_digest.into());
                                                }
                                                // Expand Tree protos to include individual file
                                                // digests in the locality map. Without this, the
                                                // server can't proxy reads for tree file blobs
                                                // until the background upload completes.
                                                let tree_file_digests = running_actions_manager
                                                    .expand_tree_file_digests(&action_result)
                                                    .await;
                                                output_digests.extend(tree_file_digests.into_iter().map(Into::into));

                                                if !output_digests.is_empty() {
                                                    let load = get_cpu_load_pct();
                                                    let p_load = get_p_core_load_pct();
                                                    let e_load = get_e_core_load_pct();
                                                    debug!("BlobsAvailable cpu_load_pct={load} p_core={p_load} e_core={e_load}");
                                                    if let Err(err) = grpc_client.blobs_available(
                                                        BlobsAvailableNotification {
                                                            worker_cas_endpoint: cas_endpoint_for_notify.clone(),
                                                            digests: output_digests,
                                                            is_full_snapshot: false,
                                                            evicted_digests: Vec::new(),
                                                            digest_infos: Vec::new(),
                                                            cpu_load_pct: load,
                                                            cached_directory_digests: Vec::new(),
                                                            added_subtree_digests: Vec::new(),
                                                            removed_subtree_digests: Vec::new(),
                                                            is_full_subtree_snapshot: false,
                                                            p_core_load_pct: p_load,
                                                            e_core_load_pct: e_load,
                                                        }
                                                    ).await {
                                                        warn!(?err, "Failed to send blobs_available notification");
                                                    }
                                                }
                                            }

                                            // 5. Upload output blobs from local CAS to remote
                                            //    CAS in the background. This is fire-and-forget;
                                            //    peers can already serve the blobs directly.
                                            running_actions_manager.spawn_upload_to_remote(&action_result);
                                        },
                                        Err(e) => {
                                            // Still notify completion on error so the worker
                                            // is freed for new work.
                                            drop(grpc_client.execution_complete(complete).await);

                                            // Only convert to FAILED_PRECONDITION if this
                                            // is a CAS blob miss (from FastSlowStore). Other
                                            // NotFound errors (e.g., command binary not found,
                                            // missing output files) should propagate as-is.
                                            let err_msg = format!("{e:?}");
                                            if e.code == Code::NotFound
                                                && err_msg.contains("not found in")
                                            {
                                                // Per REAPI spec, missing inputs should return
                                                // FAILED_PRECONDITION so the client re-uploads.
                                                warn!(
                                                    ?e,
                                                    "Missing CAS inputs, returning FAILED_PRECONDITION"
                                                );
                                                let action_result = ActionResult {
                                                    error: Some(make_err!(
                                                        Code::FailedPrecondition,
                                                        "{}",
                                                        e.message_string()
                                                    )),
                                                    ..ActionResult::default()
                                                };
                                                let action_stage = ActionStage::Completed(action_result);
                                                grpc_client.execution_response(ExecuteResult{
                                                    instance_name,
                                                    operation_id,
                                                    result: Some(execute_result::Result::ExecuteResponse(action_stage.into())),
                                                }).await.err_tip(|| "Error calling execution_response with missing inputs")?;
                                            } else {
                                                grpc_client.execution_response(ExecuteResult{
                                                    instance_name,
                                                    operation_id,
                                                    result: Some(execute_result::Result::InternalError(e.into())),
                                                }).await.err_tip(|| "Error calling execution_response with error")?;
                                            }
                                        },
                                    }
                                    Ok(())
                                }
                            };

                            self.actions_in_transit.fetch_add(1, Ordering::Release);

                            let add_future_channel = add_future_channel.clone();

                            info_span!(
                                "worker_start_action_ctx",
                                operation_id = operation_id_to_log,
                                digest_function = %digest_hasher.to_string(),
                            ).in_scope(|| {
                                let _guard = Context::current_with_value(digest_hasher)
                                    .attach();

                                let actions_in_flight = actions_in_flight.clone();
                                let actions_notify = actions_notify.clone();
                                let actions_in_flight_fail = actions_in_flight.clone();
                                let actions_notify_fail = actions_notify.clone();
                                actions_in_flight.fetch_add(1, Ordering::Release);

                                futures.push(
                                    spawn!("worker_start_action", start_action_fut).map(move |res| {
                                        let res = res.err_tip(|| "Failed to launch spawn")?;
                                        if let Err(err) = &res {
                                            error!(?err, "Error executing action");
                                        }
                                        add_future_channel
                                            .send(make_publish_future(res).then(move |res| {
                                                actions_in_flight.fetch_sub(1, Ordering::Release);
                                                actions_notify.notify_one();
                                                core::future::ready(res)
                                            }).boxed())
                                            .map_err(|_| make_err!(Code::Internal, "LocalWorker could not send future"))?;
                                        Ok(())
                                    })
                                    .or_else(move |err| {
                                        // If the make_publish_future is not run we still need to notify.
                                        actions_in_flight_fail.fetch_sub(1, Ordering::Release);
                                        actions_notify_fail.notify_one();
                                        core::future::ready(Err(err))
                                    })
                                    .boxed()
                                );
                            });
                        }
                    }
                },
                res = add_future_rx.next() => {
                    let fut = res.err_tip(|| "New future stream receives should never be closed")?;
                    futures.push(fut);
                },
                res = futures.next() => res.err_tip(|| "Keep-alive should always pending. Likely unable to send data to scheduler")??,
                complete_msg = shutdown_rx.recv().fuse() => {
                    warn!("Worker loop received shutdown signal. Shutting down worker...",);
                    let mut grpc_client = self.grpc_client.clone();
                    let shutdown_guard = complete_msg.map_err(|e| make_err!(Code::Internal, "Failed to receive shutdown message: {e:?}"))?;
                    let actions_in_flight = actions_in_flight.clone();
                    let actions_notify = actions_notify.clone();
                    let shutdown_future = async move {
                        // Wait for in-flight operations to be fully completed.
                        while actions_in_flight.load(Ordering::Acquire) > 0 {
                            actions_notify.notified().await;
                        }
                        // Sending this message immediately evicts all jobs from
                        // this worker, of which there should be none.
                        if let Err(e) = grpc_client.going_away(GoingAwayRequest {}).await {
                            error!("Failed to send GoingAwayRequest: {e}",);
                            return Err(e);
                        }
                        // Allow shutdown to occur now.
                        drop(shutdown_guard);
                        Ok::<(), Error>(())
                    };
                    futures.push(shutdown_future.boxed());
                    shutting_down = true;
                },
            };
        }
        // Unreachable.
    }
}

type ConnectionFactory<T> = Box<dyn Fn() -> BoxFuture<'static, Result<T, Error>> + Send + Sync>;

pub struct LocalWorker<T: WorkerApiClientTrait + 'static, U: RunningActionsManager> {
    config: Arc<LocalWorkerConfig>,
    running_actions_manager: Arc<U>,
    connection_factory: ConnectionFactory<T>,
    sleep_fn: Option<Box<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>>,
    metrics: Arc<Metrics>,
    /// State for periodic BlobsAvailable reporting.
    blobs_available_state: Option<BlobsAvailableState>,
    /// Guards for the worker CAS server tasks (TCP + QUIC). Keeps the tasks
    /// alive as long as the `LocalWorker` is alive. When dropped, servers abort.
    _cas_server_guards: Vec<JoinHandleDropGuard<Result<(), Error>>>,
}

impl<
    T: WorkerApiClientTrait + core::fmt::Debug + 'static,
    U: RunningActionsManager + core::fmt::Debug,
> core::fmt::Debug for LocalWorker<T, U>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LocalWorker")
            .field("config", &self.config)
            .field("running_actions_manager", &self.running_actions_manager)
            .field("metrics", &self.metrics)
            .finish_non_exhaustive()
    }
}

/// Creates a new `LocalWorker`. The `cas_store` must be an instance of
/// `FastSlowStore` and will be checked at runtime.
pub async fn new_local_worker(
    config: Arc<LocalWorkerConfig>,
    cas_store: Store,
    ac_store: Option<Store>,
    historical_store: Store,
) -> Result<LocalWorker<WorkerApiClientWrapper, RunningActionsManagerImpl>, Error> {
    start_cpu_sampler()?;

    let fast_slow_store = cas_store
        .downcast_ref::<FastSlowStore>(None)
        .err_tip(|| "Expected store for LocalWorker's store to be a FastSlowStore")?
        .get_arc()
        .err_tip(|| "FastSlowStore's Arc doesn't exist")?;

    // Log warning about CAS configuration for multi-worker setups
    event!(
        Level::INFO,
        worker_name = %config.name,
        "Starting worker '{}'. IMPORTANT: If running multiple workers, all workers \
        must share the same CAS storage path to avoid 'Object not found' errors.",
        config.name
    );

    if let Ok(path) = fs::canonicalize(&config.work_directory).await {
        fs::remove_dir_all(&path).await.err_tip(|| {
            format!(
                "Could not remove work_directory '{}' in LocalWorker",
                &path.as_path().to_str().unwrap_or("bad path")
            )
        })?;
    }

    fs::create_dir_all(&config.work_directory)
        .await
        .err_tip(|| format!("Could not make work_directory : {}", config.work_directory))?;
    let entrypoint = if config.entrypoint.is_empty() {
        None
    } else {
        Some(config.entrypoint.clone())
    };
    let max_action_timeout = if config.max_action_timeout == 0 {
        DEFAULT_MAX_ACTION_TIMEOUT
    } else {
        Duration::from_secs(config.max_action_timeout as u64)
    };
    let max_upload_timeout = if config.max_upload_timeout == 0 {
        DEFAULT_MAX_UPLOAD_TIMEOUT
    } else {
        Duration::from_secs(config.max_upload_timeout as u64)
    };

    // If peer blob sharing is configured (cas_server_port is set), create a
    // worker-local locality map and wrap the slow store with WorkerProxyStore.
    // This enables workers to fetch blobs from peers instead of the central CAS.
    let (effective_cas_store, peer_locality_map) = if config.cas_server_port.is_some() {
        let locality_map = nativelink_util::blob_locality_map::new_shared_blob_locality_map();

        // Wrap the slow store (central CAS) with WorkerProxyStore.
        // Enable racing so the worker races peer fetches against server fetches.
        let slow_store = fast_slow_store.slow_store().clone();
        let mut proxy_arc =
            nativelink_store::worker_proxy_store::WorkerProxyStore::new(
                slow_store,
                locality_map.clone(),
            );
        Arc::get_mut(&mut proxy_arc)
            .expect("WorkerProxyStore just created, no other refs")
            .enable_race_peers();
        let proxy_store = Store::new(proxy_arc);

        // Build a new FastSlowStore: fast=local disk, slow=WorkerProxyStore(central CAS).
        // Preserve the original store's direction config so that e.g.
        // slow_direction=get prevents uploads from propagating to the server.
        let fast_store = fast_slow_store.fast_store().clone();
        let fss_spec = nativelink_config::stores::FastSlowSpec {
            fast: nativelink_config::stores::StoreSpec::Noop(Default::default()),
            slow: nativelink_config::stores::StoreSpec::Noop(Default::default()),
            fast_direction: fast_slow_store.fast_direction(),
            slow_direction: fast_slow_store.slow_direction(),
        };
        let new_fss = FastSlowStore::new(&fss_spec, fast_store, proxy_store);
        info!(
            "Peer blob sharing enabled: wrapping slow store with WorkerProxyStore"
        );

        (new_fss, Some(locality_map))
    } else {
        (fast_slow_store.clone(), None)
    };

    // Initialize directory cache if configured.
    // This is done after effective_cas_store is created so the cache can use
    // the same FastSlowStore (with WorkerProxyStore) for batch downloads.
    let directory_cache = if let Some(cache_config) = &config.directory_cache {
        use std::path::PathBuf;

        use crate::directory_cache::{
            DirectoryCache, DirectoryCacheConfig as WorkerDirCacheConfig,
        };

        let cache_root = if cache_config.cache_root.is_empty() {
            PathBuf::from(&config.work_directory).parent().map_or_else(
                || PathBuf::from("/tmp/nativelink_directory_cache"),
                |p| p.join("directory_cache"),
            )
        } else {
            PathBuf::from(&cache_config.cache_root)
        };

        let worker_cache_config = WorkerDirCacheConfig {
            max_entries: cache_config.max_entries,
            max_size_bytes: cache_config.max_size_bytes,
            cache_root,
            direct_use_mode: cache_config.direct_use_mode,
        };

        match DirectoryCache::new(
            worker_cache_config,
            Store::new(effective_cas_store.clone()),
            Some(effective_cas_store.clone()),
        ).await {
            Ok(cache) => {
                tracing::info!("Directory cache initialized successfully");
                Some(Arc::new(cache))
            }
            Err(e) => {
                tracing::warn!("Failed to initialize directory cache: {:?}", e);
                None
            }
        }
    } else {
        None
    };

    let effective_cas_store_for_cas_server = effective_cas_store.clone();

    let running_actions_manager =
        Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
            root_action_directory: config.work_directory.clone(),
            execution_configuration: ExecutionConfiguration {
                entrypoint,
                additional_environment: config.additional_environment.clone(),
            },
            cas_store: effective_cas_store,
            ac_store,
            historical_store,
            upload_action_result_config: &config.upload_action_result,
            max_action_timeout,
            max_upload_timeout,
            timeout_handled_externally: config.timeout_handled_externally,
            directory_cache,
            peer_locality_map: peer_locality_map.clone(),
        })?);

    // Set up BlobsAvailable reporting with drain-then-fire semantics.
    // The send loop wakes immediately on blob insert/eviction via Notify,
    // with a backstop interval to catch subtree-only changes.
    let blobs_available_state = if config.cas_server_port.is_some() {
        // Try to get a reference to the FilesystemStore (the fast store in FastSlowStore).
        let fs_store_opt: Option<Arc<FilesystemStore>> = fast_slow_store
            .fast_store()
            .downcast_ref::<FilesystemStore>(None)
            .and_then(|fs| fs.get_arc());

        if let Some(fs_store) = fs_store_opt {
            let max_interval_ms = if config.blobs_available_interval_ms == 0 {
                BLOBS_AVAILABLE_MAX_INTERVAL_MS
            } else {
                config.blobs_available_interval_ms
            };
            let cas_endpoint = config
                .cas_server_port
                .map(|port| cas_advertised_endpoint(port))
                .unwrap_or_default();

            // Shared notify: tracker fires it on insert/eviction, send loop
            // awaits it to wake immediately.
            let notify = Arc::new(Notify::new());

            // Create change tracker and register it on the FilesystemStore.
            let tracker = BlobChangeTracker::new(notify.clone());
            if let Err(err) = fs_store
                .clone()
                .register_item_callback(tracker.clone())
            {
                warn!(?err, "Failed to register blob change tracker on FilesystemStore");
            } else {
                info!(
                    max_interval_ms,
                    "Registered BlobsAvailable drain-then-fire reporting with callback-based change tracking"
                );
            }

            Some(BlobsAvailableState {
                fs_store,
                tracker,
                cas_endpoint,
                notify,
                max_interval: Duration::from_millis(max_interval_ms),
            })
        } else {
            warn!("FastSlowStore's fast store is not a FilesystemStore; BlobsAvailable reporting disabled");
            None
        }
    } else {
        None
    };

    // Start a CAS + ByteStream gRPC server for peer blob sharing if configured.
    // Serves the effective_cas_store (which includes WorkerProxyStore) so that
    // reads can be proxied to peers when the local store doesn't have the blob.
    let cas_server_guard = if let Some(cas_port) = config.cas_server_port {
        let cas_store = Store::new(effective_cas_store_for_cas_server);
        let store_manager = Arc::new(nativelink_store::store_manager::StoreManager::new());
        store_manager.add_store("worker_cas", cas_store);

        let cas_configs = vec![nativelink_config::cas_server::WithInstanceName {
            instance_name: String::new(),
            config: nativelink_config::cas_server::CasStoreConfig {
                cas_store: "worker_cas".to_string(),
            },
        }];
        let bytestream_configs = vec![nativelink_config::cas_server::WithInstanceName {
            instance_name: String::new(),
            config: nativelink_config::cas_server::ByteStreamConfig {
                cas_store: "worker_cas".to_string(),
                ..Default::default()
            },
        }];

        let cas_server = nativelink_service::cas_server::CasServer::new(&cas_configs, &store_manager)
            .err_tip(|| "Failed to create worker CAS server")?;
        let bytestream_server =
            nativelink_service::bytestream_server::ByteStreamServer::new(&bytestream_configs, &store_manager)
                .err_tip(|| "Failed to create worker ByteStream server")?;

        let addr: std::net::SocketAddr = ([0, 0, 0, 0], cas_port).into();
        let advertised = cas_advertised_endpoint(cas_port);

        let worker_name = config.name.clone();

        // Match the main server's message size limits so that mirror writes
        // from WorkerProxyStore (which may send BatchUpdateBlobs >4MiB) are
        // not rejected by tonic's default 4MiB limit.
        const WORKER_CAS_MAX_DECODING_MESSAGE_SIZE: usize = 64 * 1024 * 1024;
        const WORKER_CAS_MAX_ENCODING_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

        // Build tonic service wrappers first (they wrap in Arc internally
        // and implement Clone), so we can share them between TCP and QUIC.
        let cas_svc = cas_server
            .into_service()
            .max_decoding_message_size(WORKER_CAS_MAX_DECODING_MESSAGE_SIZE)
            .max_encoding_message_size(WORKER_CAS_MAX_ENCODING_MESSAGE_SIZE);
        let bs_svc = bytestream_server
            .into_service()
            .max_decoding_message_size(WORKER_CAS_MAX_DECODING_MESSAGE_SIZE)
            .max_encoding_message_size(WORKER_CAS_MAX_ENCODING_MESSAGE_SIZE);

        // Start TCP server.
        let tcp_cas_svc = cas_svc.clone();
        let tcp_bs_svc = bs_svc.clone();
        let tcp_worker_name = worker_name.clone();
        let tcp_guard = spawn!("worker_cas_tcp", async move {
            info!(
                worker_name = %tcp_worker_name,
                %addr,
                %advertised,
                "Starting worker CAS TCP server for peer blob sharing"
            );
            let result = tonic::transport::Server::builder()
                .add_service(tcp_cas_svc)
                .add_service(tcp_bs_svc)
                .serve(addr)
                .await
                .map_err(|e| make_err!(Code::Internal, "Worker CAS TCP server failed: {e:?}"));
            if let Err(ref e) = result {
                error!(%addr, ?e, "Worker CAS TCP server exited with error");
            }
            result
        });

        // Start QUIC/H3 server on the same port (UDP) for peer blob sharing.
        #[cfg(feature = "quic")]
        let _quic_guard = {
            let quic_routes = tonic::service::Routes::new(cas_svc).add_service(bs_svc);
            match start_worker_quic_server(cas_port, &worker_name, quic_routes) {
                Ok(guard) => Some(guard),
                Err(e) => {
                    warn!(?e, "Failed to start worker QUIC CAS server, falling back to TCP only");
                    None
                }
            }
        };

        #[allow(unused_mut)]
        let mut guards = vec![tcp_guard];
        #[cfg(feature = "quic")]
        if let Some(quic_guard) = _quic_guard {
            guards.push(quic_guard);
        }
        guards
    } else {
        Vec::new()
    };

    // Start pprof HTTP server if configured and the feature is enabled.
    #[cfg(feature = "pprof")]
    if config.pprof_port != 0 {
        match nativelink_util::pprof_server::start_pprof_server(config.pprof_port) {
            Ok(guard) => {
                // Leak the guard so the server lives for the process lifetime.
                // The pprof server is a diagnostic tool that should outlive any
                // individual worker reconnection cycle.
                std::mem::forget(guard);
                info!(port = config.pprof_port, "pprof HTTP server started");
            }
            Err(e) => {
                warn!(?e, port = config.pprof_port, "failed to start pprof HTTP server");
            }
        }
    }

    let local_worker = LocalWorker::new_with_connection_factory_and_actions_manager(
        config.clone(),
        running_actions_manager,
        Box::new(move || {
            let config = config.clone();
            Box::pin(async move {
                // Check if QUIC/HTTP3 is requested for the worker API endpoint.
                #[cfg(feature = "quic")]
                if config.worker_api_endpoint.use_http3 {
                    let grpc_endpoint = nativelink_config::stores::GrpcEndpoint {
                        address: config.worker_api_endpoint.uri.clone(),
                        tls_config: None,
                        concurrency_limit: None,
                        connect_timeout_s: 0,
                        tcp_keepalive_s: 0,
                        http2_keepalive_interval_s: 0,
                        http2_keepalive_timeout_s: 0,
                        tcp_nodelay: true,
                        use_http3: true,
                    };
                    let quic_channel = tls_utils::h3_channel(&grpc_endpoint)
                        .map_err(|e| make_err!(
                            Code::Internal,
                            "Failed to create QUIC channel for worker API: {e:?}"
                        ))?;
                    info!(
                        uri = %config.worker_api_endpoint.uri,
                        "Worker API: using QUIC/HTTP3 transport"
                    );
                    return Ok(WorkerApiClient::new(quic_channel).into());
                }

                let timeout = config
                    .worker_api_endpoint
                    .timeout
                    .unwrap_or(DEFAULT_ENDPOINT_TIMEOUT_S);
                let timeout_duration = Duration::from_secs_f32(timeout);
                let tls_config =
                    tls_utils::load_client_config(&config.worker_api_endpoint.tls_config)
                        .err_tip(|| "Parsing local worker TLS configuration")?;
                let endpoint =
                    tls_utils::endpoint_from(&config.worker_api_endpoint.uri, tls_config)
                        .map_err(|e| make_input_err!("Invalid URI for worker endpoint : {e:?}"))?
                        .connect_timeout(timeout_duration)
                        .timeout(timeout_duration);

                let transport = endpoint.connect().await.map_err(|e| {
                    make_err!(
                        Code::Internal,
                        "Could not connect to endpoint {}: {e:?}",
                        config.worker_api_endpoint.uri
                    )
                })?;
                Ok(WorkerApiClient::new(transport).into())
            })
        }),
        Box::new(move |d| Box::pin(sleep(d))),
        blobs_available_state,
        cas_server_guard,
    );
    Ok(local_worker)
}

impl<T: WorkerApiClientTrait + 'static, U: RunningActionsManager> LocalWorker<T, U> {
    pub fn new_with_connection_factory_and_actions_manager(
        config: Arc<LocalWorkerConfig>,
        running_actions_manager: Arc<U>,
        connection_factory: ConnectionFactory<T>,
        sleep_fn: Box<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>,
        blobs_available_state: Option<BlobsAvailableState>,
        cas_server_guards: Vec<JoinHandleDropGuard<Result<(), Error>>>,
    ) -> Self {
        let metrics = Arc::new(Metrics::new(Arc::downgrade(
            running_actions_manager.metrics(),
        )));
        Self {
            config,
            running_actions_manager,
            connection_factory,
            sleep_fn: Some(sleep_fn),
            metrics,
            blobs_available_state,
            _cas_server_guards: cas_server_guards,
        }
    }

    #[allow(
        clippy::missing_const_for_fn,
        reason = "False positive on stable, but not on nightly"
    )]
    pub fn name(&self) -> &String {
        &self.config.name
    }

    async fn register_worker(
        &self,
        client: &mut T,
    ) -> Result<(String, Streaming<UpdateForWorker>), Error> {
        let mut extra_envs: HashMap<String, String> = HashMap::new();
        if let Some(ref additional_environment) = self.config.additional_environment {
            for (name, source) in additional_environment {
                let value = match source {
                    EnvironmentSource::Value(value) => Cow::Borrowed(value.as_str()),
                    EnvironmentSource::FromEnvironment => {
                        Cow::Owned(env::var(name).unwrap_or_default())
                    }
                    other => {
                        debug!(
                            ?other,
                            "Worker registration doesn't support this type of additional environment"
                        );
                        continue;
                    }
                };
                extra_envs.insert(name.clone(), value.into_owned());
            }
        }

        let cas_endpoint = self
            .config
            .cas_server_port
            .map_or_else(String::new, |port| cas_advertised_endpoint(port));
        let connect_worker_request = make_connect_worker_request(
            self.config.name.clone(),
            &self.config.platform_properties,
            &extra_envs,
            self.config.max_inflight_tasks,
            cas_endpoint,
        )
        .await?;
        let mut update_for_worker_stream = client
            .connect_worker(connect_worker_request)
            .await
            .err_tip(|| "Could not call connect_worker() in worker")?
            .into_inner();

        let first_msg_update = update_for_worker_stream
            .next()
            .await
            .err_tip(|| "Got EOF expected UpdateForWorker")?
            .err_tip(|| "Got error when receiving UpdateForWorker")?
            .update;

        let worker_id = match first_msg_update {
            Some(Update::ConnectionResult(connection_result)) => connection_result.worker_id,
            other => {
                return Err(make_input_err!(
                    "Expected first response from scheduler to be a ConnectResult got : {:?}",
                    other
                ));
            }
        };
        Ok((worker_id, update_for_worker_stream))
    }

    #[instrument(skip(self), level = Level::INFO)]
    pub async fn run(
        mut self,
        mut shutdown_rx: broadcast::Receiver<ShutdownGuard>,
    ) -> Result<(), Error> {
        let sleep_fn = self
            .sleep_fn
            .take()
            .err_tip(|| "Could not unwrap sleep_fn in LocalWorker::run")?;
        let sleep_fn_pin = Pin::new(&sleep_fn);
        let error_handler = Box::pin(move |err| async move {
            error!(?err, "Error");
            (sleep_fn_pin)(Duration::from_secs_f32(CONNECTION_RETRY_DELAY_S)).await;
        });

        loop {
            // First connect to our endpoint.
            let mut client = match (self.connection_factory)().await {
                Ok(client) => client,
                Err(e) => {
                    (error_handler)(e).await;
                    continue; // Try to connect again.
                }
            };

            // Next register our worker with the scheduler.
            let (inner, update_for_worker_stream) = match self.register_worker(&mut client).await {
                Err(e) => {
                    (error_handler)(e).await;
                    continue; // Try to connect again.
                }
                Ok((worker_id, update_for_worker_stream)) => (
                    LocalWorkerImpl::new(
                        &self.config,
                        client,
                        worker_id,
                        self.running_actions_manager.clone(),
                        self.metrics.clone(),
                        self.blobs_available_state.clone(),
                    ),
                    update_for_worker_stream,
                ),
            };
            info!(
                worker_id = %inner.worker_id,
                "Worker registered with scheduler"
            );

            // Now listen for connections and run all other services.
            if let Err(err) = inner.run(update_for_worker_stream, &mut shutdown_rx).await {
                'no_more_actions: {
                    // Ensure there are no actions in transit before we try to kill
                    // all our actions.
                    const ITERATIONS: usize = 1_000;

                    const ERROR_MSG: &str = "Actions in transit did not reach zero before we disconnected from the scheduler";

                    let sleep_duration = ACTIONS_IN_TRANSIT_TIMEOUT_S / ITERATIONS as f32;
                    for _ in 0..ITERATIONS {
                        if inner.actions_in_transit.load(Ordering::Acquire) == 0 {
                            break 'no_more_actions;
                        }
                        (sleep_fn_pin)(Duration::from_secs_f32(sleep_duration)).await;
                    }
                    // Don't terminate the worker process — fall through to
                    // kill_all + reconnect. The stuck create_and_add_action
                    // futures will be cancelled when kill_all drops them.
                    warn!(ERROR_MSG);
                }
                error!(?err, "Worker disconnected from scheduler");
                // Kill off any existing actions because if we re-connect, we'll
                // get some more and it might resource lock us.
                self.running_actions_manager.kill_all().await;

                (error_handler)(err).await; // Try to connect again.
            }
        }
        // Unreachable.
    }
}

#[derive(Debug, MetricsComponent)]
pub struct Metrics {
    #[metric(
        help = "Total number of actions sent to this worker to process. This does not mean it started them, it just means it received a request to execute it."
    )]
    start_actions_received: CounterWithTime,
    #[metric(help = "Total number of disconnects received from the scheduler.")]
    disconnects_received: CounterWithTime,
    #[metric(help = "Total number of keep-alives received from the scheduler.")]
    keep_alives_received: CounterWithTime,
    #[metric(
        help = "Stats about the calls to check if an action satisfies the config supplied script."
    )]
    preconditions: AsyncCounterWrapper,
    #[metric]
    #[allow(
        clippy::struct_field_names,
        reason = "TODO Fix this. Triggers on nightly"
    )]
    running_actions_manager_metrics: Weak<RunningActionManagerMetrics>,
}

impl RootMetricsComponent for Metrics {}

impl Metrics {
    fn new(running_actions_manager_metrics: Weak<RunningActionManagerMetrics>) -> Self {
        Self {
            start_actions_received: CounterWithTime::default(),
            disconnects_received: CounterWithTime::default(),
            keep_alives_received: CounterWithTime::default(),
            preconditions: AsyncCounterWrapper::default(),
            running_actions_manager_metrics,
        }
    }
}

impl Metrics {
    async fn wrap<U, T: Future<Output = U>, F: FnOnce(Arc<Self>) -> T>(
        self: Arc<Self>,
        fut: F,
    ) -> U {
        fut(self).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nativelink_util::common::DigestInfo;
    use nativelink_util::store_trait::StoreKey;

    #[test]
    fn test_blob_change_tracker_eviction_collects_and_swaps() {
        let tracker = BlobChangeTracker::new(Arc::new(Notify::new()));
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);

        // Evict two digests via the callback.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(tracker.callback(StoreKey::Digest(d1)));
        rt.block_on(tracker.callback(StoreKey::Digest(d2)));

        // Swap should return both as evicted.
        let changes = tracker.swap();
        assert!(changes.added.is_empty(), "Expected no added digests");
        assert_eq!(changes.evicted.len(), 2, "Expected 2 evicted digests");
        assert!(changes.evicted.contains(&d1), "Expected d1 in evicted set");
        assert!(changes.evicted.contains(&d2), "Expected d2 in evicted set");

        // Second swap should return empty.
        let changes2 = tracker.swap();
        assert!(changes2.added.is_empty());
        assert!(changes2.evicted.is_empty());
    }

    #[test]
    fn test_blob_change_tracker_ignores_non_digest_keys() {
        let tracker = BlobChangeTracker::new(Arc::new(Notify::new()));

        // Evict callback with a string key.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(tracker.callback(StoreKey::Str(Cow::Borrowed("some_key"))));

        // Insert callback with a string key.
        tracker.on_insert(StoreKey::Str(Cow::Borrowed("other_key")), 42);

        let changes = tracker.swap();
        assert!(changes.added.is_empty());
        assert!(changes.evicted.is_empty());
    }

    #[test]
    fn test_blob_change_tracker_insert_callback() {
        let tracker = BlobChangeTracker::new(Arc::new(Notify::new()));
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        tracker.on_insert(StoreKey::Digest(d1), 100);
        tracker.on_insert(StoreKey::Digest(d2), 200);

        let changes = tracker.swap();
        assert_eq!(changes.added.len(), 2, "Expected 2 added digests");
        // Timestamps should be approximately "now" (within 2 seconds).
        let ts1 = *changes.added.get(&d1).unwrap();
        let ts2 = *changes.added.get(&d2).unwrap();
        assert!((ts1 - now).abs() < 2, "d1 timestamp {ts1} too far from now {now}");
        assert!((ts2 - now).abs() < 2, "d2 timestamp {ts2} too far from now {now}");
        assert!(changes.evicted.is_empty());
    }

    #[test]
    fn test_blob_change_tracker_swap_returns_and_clears() {
        let tracker = BlobChangeTracker::new(Arc::new(Notify::new()));
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);

        // Accumulate an insert and an eviction.
        tracker.on_insert(StoreKey::Digest(d1), 100);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(tracker.callback(StoreKey::Digest(d2)));

        // First swap returns the accumulated changes.
        let changes = tracker.swap();
        assert_eq!(changes.added.len(), 1);
        assert!(changes.added.contains_key(&d1));
        assert_eq!(changes.evicted.len(), 1);
        assert!(changes.evicted.contains(&d2));

        // Second swap should be empty.
        let changes2 = tracker.swap();
        assert!(changes2.added.is_empty());
        assert!(changes2.evicted.is_empty());
    }

    #[test]
    fn test_blob_change_tracker_insert_then_evict_records_eviction() {
        let tracker = BlobChangeTracker::new(Arc::new(Notify::new()));
        let d1 = DigestInfo::new([1u8; 32], 100);

        // Insert then evict the same digest — the eviction must still be
        // recorded so the server knows the blob is no longer available.
        tracker.on_insert(StoreKey::Digest(d1), 100);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(tracker.callback(StoreKey::Digest(d1)));

        let changes = tracker.swap();
        // The digest was inserted then evicted within the same tick.
        // It should be removed from `added` (no longer available) and
        // appear in `evicted` so the server is notified.
        assert!(
            !changes.added.contains_key(&d1),
            "Expected d1 to NOT be in added after insert+evict"
        );
        assert!(
            changes.evicted.contains(&d1),
            "Expected d1 in evicted (it was evicted, removing it from added)"
        );
    }

    #[test]
    fn test_blob_change_tracker_evict_then_reinsert_cancels_out() {
        let tracker = BlobChangeTracker::new(Arc::new(Notify::new()));
        let d1 = DigestInfo::new([1u8; 32], 100);

        // Evict then reinsert the same digest — should show as added only.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(tracker.callback(StoreKey::Digest(d1)));
        tracker.on_insert(StoreKey::Digest(d1), 100);

        let changes = tracker.swap();
        assert!(
            changes.added.contains_key(&d1),
            "Expected d1 in added after evict+reinsert"
        );
        assert!(
            !changes.evicted.contains(&d1),
            "Expected d1 NOT in evicted after evict+reinsert"
        );
    }

    // ---------------------------------------------------------------
    // Gap 4: BlobChangeTracker <-> EvictingMap integration test
    // ---------------------------------------------------------------
    // Wires: EvictingMap -> ItemCallbackHolder -> BlobChangeTracker
    // and verifies that inserts and evictions flow through correctly.
    #[test]
    fn test_blob_change_tracker_evicting_map_integration() {
        use std::time::SystemTime;

        use nativelink_config::stores::EvictionPolicy;
        use nativelink_store::callback_utils::ItemCallbackHolder;
        use nativelink_util::evicting_map::{EvictingMap, LenEntry};
        use nativelink_util::store_trait::StoreKeyBorrow;

        // Simple value type for the EvictingMap.
        #[derive(Clone, Debug)]
        struct TestValue(u64);

        impl LenEntry for TestValue {
            fn len(&self) -> u64 {
                self.0
            }
            fn is_empty(&self) -> bool {
                self.0 == 0
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        rt.block_on(async {
            // Create an EvictingMap with max_bytes = 100.
            let evicting_map = EvictingMap::<
                StoreKeyBorrow,
                StoreKey<'static>,
                TestValue,
                SystemTime,
                ItemCallbackHolder,
            >::new(
                &EvictionPolicy {
                    max_count: 0,
                    max_seconds: 0,
                    max_bytes: 100,
                    evict_bytes: 0,
                },
                SystemTime::now(),
            );

            // Create a BlobChangeTracker and register it.
            let tracker = BlobChangeTracker::new(Arc::new(Notify::new()));
            let holder = ItemCallbackHolder::new(tracker.clone());
            evicting_map.add_item_callback(holder);

            let d1 = DigestInfo::new([1u8; 32], 30);
            let d2 = DigestInfo::new([2u8; 32], 40);

            // Insert two items (total 70 bytes, under 100 limit).
            let key1: StoreKeyBorrow = StoreKey::Digest(d1).into();
            let key2: StoreKeyBorrow = StoreKey::Digest(d2).into();
            evicting_map.insert(key1, TestValue(30)).await;
            evicting_map.insert(key2, TestValue(40)).await;

            // Swap and verify both digests appear in `added`.
            let changes = tracker.swap();
            assert_eq!(
                changes.added.len(),
                2,
                "Expected 2 added digests after initial inserts"
            );
            assert!(
                changes.added.contains_key(&d1),
                "Expected d1 in added set"
            );
            assert!(
                changes.added.contains_key(&d2),
                "Expected d2 in added set"
            );
            assert!(
                changes.evicted.is_empty(),
                "Expected no evictions yet"
            );

            // Now insert a third item (50 bytes) — total would be 120 bytes,
            // which exceeds max_bytes=100. This should trigger eviction of
            // the least recently used item (d1, 30 bytes).
            let d3 = DigestInfo::new([3u8; 32], 50);
            let key3: StoreKeyBorrow = StoreKey::Digest(d3).into();
            evicting_map.insert(key3, TestValue(50)).await;

            // Allow background tasks to run (eviction callbacks are fire-and-forget).
            tokio::task::yield_now().await;

            let changes = tracker.swap();
            assert!(
                changes.added.contains_key(&d3),
                "Expected d3 in added set after third insert"
            );
            assert!(
                changes.evicted.contains(&d1),
                "Expected d1 in evicted set (LRU eviction)"
            );
            // d2 should NOT have been evicted (total after eviction: 40 + 50 = 90 <= 100).
            assert!(
                !changes.evicted.contains(&d2),
                "Expected d2 to NOT be evicted"
            );
        });
    }

    #[test]
    fn test_cas_advertised_endpoint_format() {
        let endpoint = cas_advertised_endpoint(50081);
        assert!(
            endpoint.starts_with("grpc://"),
            "Expected endpoint to start with 'grpc://', got: {endpoint}"
        );
        assert!(
            endpoint.ends_with(":50081"),
            "Expected endpoint to end with ':50081', got: {endpoint}"
        );

        // Extract hostname and verify it's non-empty.
        let without_prefix = endpoint.strip_prefix("grpc://").unwrap();
        let hostname = without_prefix.strip_suffix(":50081").unwrap();
        assert!(
            !hostname.is_empty(),
            "Expected non-empty hostname in endpoint: {endpoint}"
        );
    }
}
