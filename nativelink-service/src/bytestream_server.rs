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

use core::convert::Into;
use core::fmt::{Debug, Formatter};
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll};
use core::time::Duration;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::{Bytes, BytesMut};
use futures::future::pending;
use futures::stream::unfold;
use futures::{Future, Stream, TryFutureExt, try_join};
use nativelink_config::cas_server::{ByteStreamConfig, InstanceName, WithInstanceName};
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, group, publish,
};
use nativelink_proto::google::bytestream::byte_stream_server::{
    ByteStream, ByteStreamServer as Server,
};
use nativelink_proto::google::bytestream::{
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use nativelink_store::grpc_store::GrpcStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_store::worker_proxy_store::WorkerProxyStore;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair_with_size,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::log_utils::throughput_mbps;
use nativelink_util::stall_detector::StallGuard;
use nativelink_util::digest_hasher::{
    DigestHasherFunc, default_digest_hasher_func, make_ctx_for_hash_func,
};
use nativelink_util::proto_stream_utils::WriteRequestStreamWrapper;
use nativelink_util::resource_info::ResourceInfo;
use nativelink_util::spawn;
use nativelink_util::store_trait::{IS_WORKER_REQUEST, REDIRECT_PREFIX, Store, StoreLike, StoreOptimizations, UploadSizeInfo};
use nativelink_util::task::JoinHandleDropGuard;
use nativelink_util::zero_copy_codec::{
    GrpcUnaryBody, ZeroCopyReadBody, ZeroCopyWriteStream, decode_unary_request,
    encode_grpc_unary_response,
};
use opentelemetry::context::FutureExt;
use parking_lot::Mutex;
use tokio::time::sleep;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Instrument, Level, debug, error, error_span, info, instrument, trace, warn};

/// If this value changes update the documentation in the config definition.
const DEFAULT_PERSIST_STREAM_ON_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// If this value changes update the documentation in the config definition.
const DEFAULT_MAX_BYTES_PER_STREAM: usize = 3 * 1024 * 1024;

/// Metrics for `ByteStream` server operations.
/// Tracks upload/download activity, throughput, and latency.
#[derive(Debug, Default)]
pub struct ByteStreamMetrics {
    /// Number of currently active uploads (includes idle streams waiting for resume)
    pub active_uploads: AtomicU64,
    /// Total number of write requests received
    pub write_requests_total: AtomicU64,
    /// Total number of successful write requests
    pub write_requests_success: AtomicU64,
    /// Total number of failed write requests
    pub write_requests_failure: AtomicU64,
    /// Total number of read requests received
    pub read_requests_total: AtomicU64,
    /// Total number of successful read requests
    pub read_requests_success: AtomicU64,
    /// Total number of failed read requests
    pub read_requests_failure: AtomicU64,
    /// Total number of `query_write_status` requests
    pub query_write_status_total: AtomicU64,
    /// Total bytes written via `ByteStream`
    pub bytes_written_total: AtomicU64,
    /// Total bytes read via `ByteStream`
    pub bytes_read_total: AtomicU64,
    /// Sum of write durations in nanoseconds (for average latency calculation)
    pub write_duration_ns: AtomicU64,
    /// Sum of read durations in nanoseconds (for average latency calculation)
    pub read_duration_ns: AtomicU64,
    /// Number of UUID collisions detected
    pub uuid_collisions: AtomicU64,
    /// Number of resumed uploads (client reconnected to existing stream)
    pub resumed_uploads: AtomicU64,
    /// Number of idle streams that timed out
    pub idle_stream_timeouts: AtomicU64,
}

impl MetricsComponent for ByteStreamMetrics {
    fn publish(
        &self,
        _kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let _enter = group!(field_metadata.name).entered();

        publish!(
            "active_uploads",
            &self.active_uploads,
            MetricKind::Counter,
            "Number of currently active uploads"
        );
        publish!(
            "write_requests_total",
            &self.write_requests_total,
            MetricKind::Counter,
            "Total write requests received"
        );
        publish!(
            "write_requests_success",
            &self.write_requests_success,
            MetricKind::Counter,
            "Total successful write requests"
        );
        publish!(
            "write_requests_failure",
            &self.write_requests_failure,
            MetricKind::Counter,
            "Total failed write requests"
        );
        publish!(
            "read_requests_total",
            &self.read_requests_total,
            MetricKind::Counter,
            "Total read requests received"
        );
        publish!(
            "read_requests_success",
            &self.read_requests_success,
            MetricKind::Counter,
            "Total successful read requests"
        );
        publish!(
            "read_requests_failure",
            &self.read_requests_failure,
            MetricKind::Counter,
            "Total failed read requests"
        );
        publish!(
            "query_write_status_total",
            &self.query_write_status_total,
            MetricKind::Counter,
            "Total query_write_status requests"
        );
        publish!(
            "bytes_written_total",
            &self.bytes_written_total,
            MetricKind::Counter,
            "Total bytes written via ByteStream"
        );
        publish!(
            "bytes_read_total",
            &self.bytes_read_total,
            MetricKind::Counter,
            "Total bytes read via ByteStream"
        );
        publish!(
            "write_duration_ns",
            &self.write_duration_ns,
            MetricKind::Counter,
            "Sum of write durations in nanoseconds"
        );
        publish!(
            "read_duration_ns",
            &self.read_duration_ns,
            MetricKind::Counter,
            "Sum of read durations in nanoseconds"
        );
        publish!(
            "uuid_collisions",
            &self.uuid_collisions,
            MetricKind::Counter,
            "Number of UUID collisions detected"
        );
        publish!(
            "resumed_uploads",
            &self.resumed_uploads,
            MetricKind::Counter,
            "Number of resumed uploads"
        );
        publish!(
            "idle_stream_timeouts",
            &self.idle_stream_timeouts,
            MetricKind::Counter,
            "Number of idle streams that timed out"
        );

        Ok(MetricPublishKnownKindData::Component)
    }
}

type BytesWrittenAndIdleStream = (Arc<AtomicU64>, Option<IdleStream>);

/// Type alias for the UUID key used in `active_uploads` `HashMap`.
/// Using u128 instead of String reduces memory allocations and improves
/// cache locality for `HashMap` operations.
type UuidKey = u128;

/// Parse a UUID string to a u128 for use as a `HashMap` key.
/// This avoids heap allocation for String keys and improves `HashMap` performance.
/// Falls back to hashing the string if it's not a valid hex UUID.
#[inline]
fn parse_uuid_to_key(uuid_str: &str) -> UuidKey {
    // UUIDs are typically 32 hex chars (128 bits) or 36 chars with dashes.
    // We'll try to parse as hex first, then fall back to hashing.
    let clean: String = uuid_str.chars().filter(char::is_ascii_hexdigit).collect();
    if clean.len() >= 16 {
        // Take up to 32 hex chars (128 bits)
        let hex_str = if clean.len() > 32 {
            &clean[..32]
        } else {
            &clean
        };
        u128::from_str_radix(hex_str, 16).unwrap_or_else(|_| {
            // Hash fallback for non-hex strings
            use core::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            uuid_str.hash(&mut hasher);
            u128::from(hasher.finish())
        })
    } else {
        // Short strings: use hash
        use core::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        uuid_str.hash(&mut hasher);
        u128::from(hasher.finish())
    }
}

pub struct InstanceInfo {
    store: Store,
    // Max number of bytes to send on each grpc stream chunk.
    max_bytes_per_stream: usize,
    /// Active uploads keyed by UUID as u128 for better performance.
    /// Using u128 keys instead of String reduces heap allocations
    /// and improves `HashMap` lookup performance.
    active_uploads: Arc<Mutex<HashMap<UuidKey, BytesWrittenAndIdleStream>>>,
    /// How long to keep idle streams before timing them out.
    idle_stream_timeout: Duration,
    metrics: Arc<ByteStreamMetrics>,
    /// Handle to the global sweeper task. Kept alive for the lifetime of the instance.
    _sweeper_handle: Arc<JoinHandleDropGuard<()>>,
    /// In-flight CAS writes keyed by digest. When multiple RPCs arrive for
    /// the same digest concurrently, only the first performs the actual
    /// write; the rest subscribe to the watch channel and get the result.
    /// `None` = in progress, `Some(true)` = succeeded, `Some(false)` = failed.
    in_flight_writes: Arc<Mutex<HashMap<DigestInfo, tokio::sync::watch::Receiver<Option<bool>>>>>,
}

impl Debug for InstanceInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InstanceInfo")
            .field("store", &self.store)
            .field("max_bytes_per_stream", &self.max_bytes_per_stream)
            .field("active_uploads", &self.active_uploads)
            .field("idle_stream_timeout", &self.idle_stream_timeout)
            .field("metrics", &self.metrics)
            .finish()
    }
}

type ReadStream = Pin<Box<dyn Stream<Item = Result<ReadResponse, Status>> + Send + 'static>>;
type StoreUpdateFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'static>>;

/// Wrapper around a `ReadStream` that logs total bytes and elapsed time when
/// the stream completes (yields `None`) or is dropped before completion.
struct LoggingReadStream {
    inner: ReadStream,
    start_time: Instant,
    digest: DigestInfo,
    expected_size: u64,
    bytes_sent: u64,
    completed: bool,
}

impl LoggingReadStream {
    fn new(inner: ReadStream, start_time: Instant, digest: DigestInfo, expected_size: u64) -> Self {
        Self {
            inner,
            start_time,
            digest,
            expected_size,
            bytes_sent: 0,
            completed: false,
        }
    }

    fn log_completion(&self, status: &str) {
        let elapsed = self.start_time.elapsed();
        let elapsed_ms = elapsed.as_millis() as u64;
        info!(
            digest = %self.digest,
            expected_size = self.expected_size,
            bytes_sent = self.bytes_sent,
            elapsed_ms,
            throughput_mbps = %throughput_mbps(self.bytes_sent, elapsed),
            status,
            "ByteStream::read: CAS read completed",
        );
    }
}

impl Stream for LoggingReadStream {
    type Item = Result<ReadResponse, Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let result = self.inner.as_mut().poll_next(cx);
        match &result {
            Poll::Ready(Some(Ok(response))) => {
                self.bytes_sent += response.data.len() as u64;
            }
            Poll::Ready(None) => {
                self.completed = true;
                self.log_completion("ok");
            }
            Poll::Ready(Some(Err(_))) => {
                self.completed = true;
                self.log_completion("error");
            }
            Poll::Pending => {}
        }
        result
    }
}

impl Drop for LoggingReadStream {
    fn drop(&mut self) {
        if !self.completed {
            self.log_completion("dropped");
        }
    }
}

struct StreamState {
    uuid: UuidKey,
    tx: DropCloserWriteHalf,
    store_update_fut: StoreUpdateFuture,
}

impl Debug for StreamState {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StreamState")
            .field("uuid", &format!("{:032x}", self.uuid))
            .finish()
    }
}

/// If a stream is in this state, it will automatically be put back into an `IdleStream` and
/// placed back into the `active_uploads` map as an `IdleStream` after it is dropped.
/// To prevent it from being put back into an `IdleStream` you must call `.graceful_finish()`.
struct ActiveStreamGuard {
    stream_state: Option<StreamState>,
    bytes_received: Arc<AtomicU64>,
    active_uploads: Arc<Mutex<HashMap<UuidKey, BytesWrittenAndIdleStream>>>,
    metrics: Arc<ByteStreamMetrics>,
}

impl ActiveStreamGuard {
    /// Consumes the guard. The stream will be considered "finished", will
    /// remove it from the `active_uploads`.
    fn graceful_finish(mut self) {
        let stream_state = self.stream_state.take().unwrap();
        self.active_uploads.lock().remove(&stream_state.uuid);
        // Decrement active uploads counter on successful completion
        self.metrics.active_uploads.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Drop for ActiveStreamGuard {
    fn drop(&mut self) {
        let Some(stream_state) = self.stream_state.take() else {
            return; // If None it means we don't want it put back into an IdleStream.
        };
        let mut active_uploads = self.active_uploads.lock();
        let uuid = stream_state.uuid; // u128 is Copy, no clone needed
        let Some(active_uploads_slot) = active_uploads.get_mut(&uuid) else {
            error!(
                err = "Failed to find active upload. This should never happen.",
                uuid = format!("{:032x}", uuid),
            );
            return;
        };
        // Mark stream as idle with current timestamp.
        // The global sweeper will clean it up after idle_stream_timeout.
        // This avoids spawning a task per stream, reducing overhead from O(n) to O(1).
        active_uploads_slot.1 = Some(IdleStream {
            stream_state,
            idle_since: Instant::now(),
        });
    }
}

/// Represents a stream that is in the "idle" state. this means it is not currently being used
/// by a client. If it is not used within a certain amount of time it will be removed from the
/// `active_uploads` map automatically by the global sweeper task.
#[derive(Debug)]
struct IdleStream {
    stream_state: StreamState,
    /// When this stream became idle. Used by the global sweeper to determine expiration.
    idle_since: Instant,
}

impl IdleStream {
    fn into_active_stream(
        self,
        bytes_received: Arc<AtomicU64>,
        instance_info: &InstanceInfo,
    ) -> ActiveStreamGuard {
        ActiveStreamGuard {
            stream_state: Some(self.stream_state),
            bytes_received,
            active_uploads: instance_info.active_uploads.clone(),
            metrics: instance_info.metrics.clone(),
        }
    }
}

/// Maximum blob size for mirroring via the streaming write path. The streaming
/// path does not buffer the data, so mirroring requires a re-read from the
/// store. We only do this for blobs <= 16MB to avoid expensive re-reads of
/// large blobs. The oneshot path passes the data directly (O(1) Bytes clone).
const MIRROR_STREAM_MAX_SIZE: u64 = 16 * 1024 * 1024;

/// Spawn a background task to mirror a blob to a random connected worker
/// for OOM redundancy. Fire-and-forget: errors are logged, not propagated.
///
/// When `data` is `Some`, the blob data is sent directly (used by the oneshot
/// and BatchUpdateBlobs paths where data is already in hand). When `None`,
/// the blob is re-read from the store (used by the streaming write path for
/// small blobs only).
fn mirror_blob_to_worker(store: &Store, digest: DigestInfo, data: Option<Bytes>) {
    // WorkerProxyStore is the outermost wrapper on CAS stores when workers
    // are configured. inner_store() delegates through, so we use as_any()
    // on the immediate store driver to find it.
    if store
        .as_store_driver()
        .as_any()
        .downcast_ref::<WorkerProxyStore>()
        .is_none()
    {
        return;
    }

    // Skip zero-length blobs — no value in mirroring them.
    if digest.size_bytes() == 0 {
        return;
    }

    let store = store.clone();
    nativelink_util::background_spawn!("mirror_blob_to_worker", async move {
        let blob_data = if let Some(d) = data {
            d
        } else {
            // Streaming path: re-read from store since we don't have the data buffered.
            match store.get_part_unchunked(digest, 0, None).await {
                Ok(d) => d,
                Err(e) => {
                    warn!(
                        %digest,
                        ?e,
                        "mirror: failed to read blob for mirroring"
                    );
                    return;
                }
            }
        };

        // Re-obtain the proxy reference (store is cloned, driver is Arc'd).
        let Some(proxy) = store
            .as_store_driver()
            .as_any()
            .downcast_ref::<WorkerProxyStore>()
        else {
            return;
        };

        proxy.mirror_blob_to_random_worker(digest, blob_data).await;
    });
}

#[derive(Debug)]
pub struct ByteStreamServer {
    instance_infos: HashMap<InstanceName, InstanceInfo>,
}

impl ByteStreamServer {
    /// Generate a unique UUID key by `XOR`ing the base key with a nanosecond timestamp.
    /// This ensures virtually zero collision probability while being O(1).
    fn generate_unique_uuid_key(base_key: UuidKey) -> UuidKey {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        // XOR with timestamp to create unique key
        base_key ^ timestamp
    }

    pub fn new(
        configs: &[WithInstanceName<ByteStreamConfig>],
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut instance_infos: HashMap<String, InstanceInfo> = HashMap::new();
        for config in configs {
            let idle_stream_timeout = if config.persist_stream_on_disconnect_timeout == 0 {
                DEFAULT_PERSIST_STREAM_ON_DISCONNECT_TIMEOUT
            } else {
                Duration::from_secs(config.persist_stream_on_disconnect_timeout as u64)
            };
            let _old_value = instance_infos.insert(
                config.instance_name.clone(),
                Self::new_with_timeout(config, store_manager, idle_stream_timeout)?,
            );
        }
        Ok(Self { instance_infos })
    }

    pub fn new_with_timeout(
        config: &WithInstanceName<ByteStreamConfig>,
        store_manager: &StoreManager,
        idle_stream_timeout: Duration,
    ) -> Result<InstanceInfo, Error> {
        let store = store_manager
            .get_store(&config.cas_store)
            .ok_or_else(|| make_input_err!("'cas_store': '{}' does not exist", config.cas_store))?;
        let max_bytes_per_stream = if config.max_bytes_per_stream == 0 {
            DEFAULT_MAX_BYTES_PER_STREAM
        } else {
            if config.max_bytes_per_stream > 4 * 1024 * 1024 {
                warn!(
                    configured = config.max_bytes_per_stream,
                    default = DEFAULT_MAX_BYTES_PER_STREAM,
                    "max_bytes_per_stream exceeds 4 MiB; Bazel and other REAPI clients \
                     typically have a 4 MiB gRPC inbound message limit and will reject \
                     oversized ByteStream.Read chunks with RESOURCE_EXHAUSTED"
                );
            }
            config.max_bytes_per_stream
        };

        let active_uploads: Arc<Mutex<HashMap<UuidKey, BytesWrittenAndIdleStream>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let metrics = Arc::new(ByteStreamMetrics::default());

        // Spawn a single global sweeper task that periodically cleans up expired idle streams.
        // This replaces per-stream timeout tasks, reducing task spawn overhead from O(n) to O(1).
        let sweeper_active_uploads = Arc::downgrade(&active_uploads);
        let sweeper_metrics = Arc::downgrade(&metrics);
        let sweep_interval = idle_stream_timeout / 2; // Check every half-timeout period
        let sweeper_handle = spawn!("bytestream_idle_stream_sweeper", async move {
            loop {
                sleep(sweep_interval).await;

                let Some(active_uploads) = sweeper_active_uploads.upgrade() else {
                    // InstanceInfo has been dropped, exit the sweeper
                    break;
                };
                let metrics = sweeper_metrics.upgrade();

                let now = Instant::now();
                let mut expired_count = 0u64;

                // Lock and sweep expired entries
                {
                    let mut uploads = active_uploads.lock();
                    uploads.retain(|uuid, (_, maybe_idle)| {
                        if let Some(idle_stream) = maybe_idle {
                            if now.duration_since(idle_stream.idle_since) >= idle_stream_timeout {
                                debug!(
                                    msg = "Sweeping expired idle stream",
                                    uuid = format!("{:032x}", uuid)
                                );
                                expired_count += 1;
                                return false; // Remove this entry
                            }
                        }
                        true // Keep this entry
                    });
                }

                // Update metrics outside the lock
                if expired_count > 0 {
                    if let Some(m) = &metrics {
                        m.idle_stream_timeouts
                            .fetch_add(expired_count, Ordering::Relaxed);
                        m.active_uploads.fetch_sub(expired_count, Ordering::Relaxed);
                    }
                    trace!(
                        msg = "Sweeper cleaned up expired streams",
                        count = expired_count
                    );
                }
            }
        });

        Ok(InstanceInfo {
            store,
            max_bytes_per_stream,
            active_uploads,
            idle_stream_timeout,
            metrics,
            _sweeper_handle: Arc::new(sweeper_handle),
            in_flight_writes: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    /// Wrap this server in a `ZeroCopyByteStreamService` that intercepts Write
    /// RPCs and decodes `WriteRequest` messages directly from HTTP body frames,
    /// bypassing tonic's `BytesMut` reassembly buffer.
    ///
    /// Read and QueryWriteStatus RPCs delegate to the standard tonic path.
    pub fn into_zero_copy_service(
        self,
        max_decoding_message_size: usize,
        max_encoding_message_size: usize,
    ) -> ZeroCopyByteStreamService {
        let inner = Arc::new(self);
        ZeroCopyByteStreamService {
            inner: inner.clone(),
            tonic_service: Server::from_arc(inner)
                .max_decoding_message_size(max_decoding_message_size)
                .max_encoding_message_size(max_encoding_message_size),
        }
    }

    /// Creates or joins an upload stream for the given UUID.
    ///
    /// This function handles three scenarios:
    /// 1. UUID doesn't exist - creates a new upload stream
    /// 2. UUID exists but is idle - resumes the existing stream
    /// 3. UUID exists and is active - generates a unique UUID by appending a nanosecond
    ///    timestamp to avoid collision, then creates a new stream with that UUID
    ///
    /// The nanosecond timestamp ensures virtually zero probability of collision since
    /// two concurrent uploads would need to both collide on the original UUID AND
    /// generate the unique UUID in the exact same nanosecond.
    fn create_or_join_upload_stream(
        &self,
        uuid_str: &str,
        instance: &InstanceInfo,
        digest: DigestInfo,
    ) -> ActiveStreamGuard {
        // Parse UUID string to u128 key for efficient HashMap operations
        let uuid_key = parse_uuid_to_key(uuid_str);

        // We handle the three cases in two phases to avoid holding the
        // mutex guard across a second .lock() call (which would deadlock
        // on parking_lot::Mutex since it is not reentrant).
        enum UploadAction {
            Resume(Box<ActiveStreamGuard>),
            New(u128, Arc<AtomicU64>),
            Collision(u128),
        }

        let action = {
            let mut active_uploads = instance.active_uploads.lock();
            match active_uploads.entry(uuid_key) {
                Entry::Occupied(mut entry) => {
                    let maybe_idle_stream = entry.get_mut();
                    if let Some(idle_stream) = maybe_idle_stream.1.take() {
                        // Case 2: Stream exists but is idle, we can resume it
                        let bytes_received = maybe_idle_stream.0.clone();
                        debug!(
                            msg = "Joining existing stream",
                            uuid = format!("{:032x}", entry.key())
                        );
                        // Track resumed upload
                        instance
                            .metrics
                            .resumed_uploads
                            .fetch_add(1, Ordering::Relaxed);
                        UploadAction::Resume(Box::new(
                            idle_stream.into_active_stream(bytes_received, instance),
                        ))
                    } else {
                        // Case 3: Stream is active - generate a unique UUID to avoid collision
                        let original_key = *entry.key();
                        let unique_key = Self::generate_unique_uuid_key(original_key);
                        warn!(
                            msg = "UUID collision detected, generating unique UUID to prevent conflict",
                            original_uuid = format!("{:032x}", original_key),
                            unique_uuid = format!("{:032x}", unique_key)
                        );
                        UploadAction::Collision(unique_key)
                    }
                }
                Entry::Vacant(entry) => {
                    // Case 1: UUID doesn't exist, create new stream
                    let bytes_received = Arc::new(AtomicU64::new(0));
                    let uuid = *entry.key();
                    entry.insert((bytes_received.clone(), None));
                    UploadAction::New(uuid, bytes_received)
                }
            }
        }; // First lock guard dropped here.

        let (uuid, bytes_received, is_collision) = match action {
            UploadAction::Resume(guard) => return *guard,
            UploadAction::New(uuid, bytes_received) => (uuid, bytes_received, false),
            UploadAction::Collision(unique_key) => {
                let bytes_received = Arc::new(AtomicU64::new(0));
                let mut active_uploads = instance.active_uploads.lock();
                active_uploads.insert(unique_key, (bytes_received.clone(), None));
                (unique_key, bytes_received, true)
            }
        };

        // Track metrics for new upload
        instance
            .metrics
            .active_uploads
            .fetch_add(1, Ordering::Relaxed);
        if is_collision {
            instance
                .metrics
                .uuid_collisions
                .fetch_add(1, Ordering::Relaxed);
        }

        // Important: Do not return an error from this point onwards without
        // removing the entry from the map, otherwise that UUID becomes
        // unusable.

        // Use a larger buffer (256 slots = ~64MiB at 256KiB chunks) to sustain
        // high-throughput streaming at 10Gbps+ without backpressure stalls.
        let (tx, rx) = make_buf_channel_pair_with_size(256);
        let store = instance.store.clone();
        let store_update_fut = Box::pin(async move {
            // We need to wrap `Store::update()` in a another future because we need to capture
            // `store` to ensure its lifetime follows the future and not the caller.
            store
                // Bytestream always uses digest size as the actual byte size.
                .update(digest, rx, UploadSizeInfo::ExactSize(digest.size_bytes()))
                .await
        });
        ActiveStreamGuard {
            stream_state: Some(StreamState {
                uuid,
                tx,
                store_update_fut,
            }),
            bytes_received,
            active_uploads: instance.active_uploads.clone(),
            metrics: instance.metrics.clone(),
        }
    }

    async fn inner_read(
        &self,
        instance: &InstanceInfo,
        digest: DigestInfo,
        read_request: ReadRequest,
        is_worker: bool,
    ) -> Result<impl Stream<Item = Result<ReadResponse, Status>> + Send + use<>, Error> {
        struct ReaderState {
            max_bytes_per_stream: usize,
            rx: DropCloserReadHalf,
            maybe_get_part_result: Option<Result<(), Error>>,
            get_part_fut: Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>,
        }

        let read_limit = u64::try_from(read_request.read_limit)
            .err_tip(|| "Could not convert read_limit to u64")?;

        // Use a larger buffer (256 slots = ~64MiB at 256KiB chunks) to sustain
        // high-throughput streaming at 10Gbps+ without backpressure stalls.
        let (tx, rx) = make_buf_channel_pair_with_size(256);

        let read_limit = if read_limit != 0 {
            Some(read_limit)
        } else {
            None
        };

        // This allows us to call a destructor when the the object is dropped.
        let store = instance.store.clone();
        let state = Some(ReaderState {
            rx,
            max_bytes_per_stream: instance.max_bytes_per_stream,
            maybe_get_part_result: None,
            get_part_fut: Box::pin(async move {
                // Propagate the worker/non-worker distinction into the store
                // layer so WorkerProxyStore can decide whether to proxy or
                // redirect.
                IS_WORKER_REQUEST
                    .scope(is_worker, async {
                        store
                            .get_part(
                                digest,
                                tx,
                                u64::try_from(read_request.read_offset)
                                    .err_tip(|| "Could not convert read_offset to u64")?,
                                read_limit,
                            )
                            .await
                    })
                    .await
            }),
        });

        let read_stream_span = error_span!("read_stream");

        Ok(Box::pin(unfold(state, move |state| {
            async {
            let mut state = state?; // If None our stream is done.
            let mut response = ReadResponse::default();
            {
                let consume_fut = state.rx.consume(Some(state.max_bytes_per_stream));
                tokio::pin!(consume_fut);
                loop {
                    tokio::select! {
                        read_result = &mut consume_fut => {
                            match read_result {
                                Ok(bytes) => {
                                    if bytes.is_empty() {
                                        // EOF.
                                        return None;
                                    }
                                    if bytes.len() > state.max_bytes_per_stream {
                                        let err = make_err!(Code::Internal, "Returned store size was larger than read size");
                                        return Some((Err(err.into()), None));
                                    }
                                    response.data = bytes;
                                    trace!(response.data = format!("<redacted len({})>", response.data.len()));
                                    break;
                                }
                                Err(mut e) => {
                                    // We may need to propagate the error from reading the data through first.
                                    // For example, the NotFound error will come through `get_part_fut`, and
                                    // will not be present in `e`, but we need to ensure we pass NotFound error
                                    // code or the client won't know why it failed.
                                    let get_part_result = if let Some(result) = state.maybe_get_part_result {
                                        result
                                    } else {
                                        // This should never be `future::pending()` if maybe_get_part_result is
                                        // not set.
                                        state.get_part_fut.await
                                    };
                                    if let Err(err) = get_part_result {
                                        e = err.merge(e);
                                    }
                                    if e.code == Code::NotFound {
                                        // Trim the error code. Not Found is quite common and we don't want to send a large
                                        // error (debug) message for something that is common. We resize to just the last
                                        // message as it will be the most relevant.
                                        e.messages.truncate(1);
                                    }
                                    // Use appropriate log level: redirects and not-found are
                                    // expected protocol behavior, not errors.
                                    let is_redirect = e.code == Code::FailedPrecondition
                                        && e.messages.iter().any(|m| m.contains(REDIRECT_PREFIX));
                                    if is_redirect {
                                        // Redirects always produce a "Sender dropped before
                                        // sending EOF" artifact because get_part returns an
                                        // error (dropping tx) instead of streaming data. Trim
                                        // to just the redirect message for a clean response.
                                        e.messages.truncate(1);
                                        info!(response = ?e);
                                    } else if e.code == Code::NotFound {
                                        info!(response = ?e);
                                    } else {
                                        error!(response = ?e);
                                    }
                                    return Some((Err(e.into()), None))
                                }
                            }
                        },
                        result = &mut state.get_part_fut => {
                            state.maybe_get_part_result = Some(result);
                            // It is non-deterministic on which future will finish in what order.
                            // It is also possible that the `state.rx.consume()` call above may not be able to
                            // respond even though the publishing future is done.
                            // Because of this we set the writing future to pending so it never finishes.
                            // The `state.rx.consume()` future will eventually finish and return either the
                            // data or an error.
                            // An EOF will terminate the `state.rx.consume()` future, but we are also protected
                            // because we are dropping the writing future, it will drop the `tx` channel
                            // which will eventually propagate an error to the `state.rx.consume()` future if
                            // the EOF was not sent due to some other error.
                            state.get_part_fut = Box::pin(pending());
                        },
                    }
                }
            }
            Some((Ok(response), Some(state)))
        }.instrument(read_stream_span.clone())
        })))
    }

    // We instrument tracing here as well as below because `stream` has a hash on it
    // that is extracted from the first stream message. If we only implemented it below
    // we would not have the hash available to us.
    #[instrument(
        ret(level = Level::DEBUG),
        level = Level::ERROR,
        skip(self, instance_info),
    )]
    async fn inner_write(
        &self,
        instance_info: &InstanceInfo,
        digest: DigestInfo,
        stream: WriteRequestStreamWrapper<impl Stream<Item = Result<WriteRequest, Status>> + Unpin>,
    ) -> Result<Response<WriteResponse>, Error> {
        async fn process_client_stream(
            mut stream: WriteRequestStreamWrapper<
                impl Stream<Item = Result<WriteRequest, Status>> + Unpin,
            >,
            tx: &mut DropCloserWriteHalf,
            outer_bytes_received: &Arc<AtomicU64>,
            expected_size: u64,
        ) -> Result<(), Error> {
            loop {
                let write_request = match stream.next().await {
                    // Code path for when client tries to gracefully close the stream.
                    // If this happens it means there's a problem with the data sent,
                    // because we always close the stream from our end before this point
                    // by counting the number of bytes sent from the client. If they send
                    // less than the amount they said they were going to send and then
                    // close the stream, we know there's a problem.
                    None => {
                        return Err(make_input_err!(
                            "Client closed stream before sending all data"
                        ));
                    }
                    // Code path for client stream error. Probably client disconnect.
                    Some(Err(err)) => return Err(err),
                    // Code path for received chunk of data.
                    Some(Ok(write_request)) => write_request,
                };

                if write_request.write_offset < 0 {
                    return Err(make_input_err!(
                        "Invalid negative write offset in write request: {}",
                        write_request.write_offset
                    ));
                }
                let write_offset = write_request.write_offset as u64;

                // If we get duplicate data because a client didn't know where
                // it left off from, then we can simply skip it.
                let data = if write_offset < tx.get_bytes_written() {
                    if (write_offset + write_request.data.len() as u64) < tx.get_bytes_written() {
                        if write_request.finish_write {
                            return Err(make_input_err!(
                                "Resumed stream finished at {} bytes when we already received {} bytes.",
                                write_offset + write_request.data.len() as u64,
                                tx.get_bytes_written()
                            ));
                        }
                        continue;
                    }
                    write_request.data.slice(
                        usize::try_from(tx.get_bytes_written() - write_offset)
                            .unwrap_or(usize::MAX)..,
                    )
                } else {
                    if write_offset != tx.get_bytes_written() {
                        // The client is trying to resume at an offset we
                        // don't have (e.g. the idle stream was swept).
                        // Return UNAVAILABLE so the client retries with
                        // QueryWriteStatus → committed_size=0 → restart.
                        return Err(make_err!(
                            Code::Unavailable,
                            "Received out of order data (write_offset {} but server has {}). \
                             Partial upload state was lost; retry from committed offset.",
                            write_offset,
                            tx.get_bytes_written()
                        ));
                    }
                    write_request.data
                };

                // Do not process EOF or weird stuff will happen.
                if !data.is_empty() {
                    // We also need to process the possible EOF branch, so we can't early return.
                    if let Err(mut err) = tx.send(data).await {
                        err.code = Code::Internal;
                        return Err(err);
                    }
                    outer_bytes_received.store(tx.get_bytes_written(), Ordering::Release);
                }

                if expected_size < tx.get_bytes_written() {
                    return Err(make_input_err!("Received more bytes than expected"));
                }
                if write_request.finish_write {
                    // Validate that we received the expected number of bytes
                    // before accepting the upload. The stream wrapper only
                    // validates on a *subsequent* poll_next after finish_write,
                    // which we never perform, so check here explicitly.
                    if tx.get_bytes_written() != expected_size {
                        return Err(make_input_err!(
                            "Client declared size {} but only sent {} bytes",
                            expected_size,
                            tx.get_bytes_written()
                        ));
                    }
                    // Gracefully close our stream.
                    tx.send_eof()
                        .err_tip(|| "Failed to send EOF in ByteStream::write")?;
                    return Ok(());
                }
                // Continue.
            }
            // Unreachable.
        }

        let uuid = stream
            .resource_info
            .uuid
            .as_ref()
            .ok_or_else(|| make_input_err!("UUID must be set if writing data"))?;
        let mut active_stream_guard =
            self.create_or_join_upload_stream(uuid, instance_info, digest);
        let expected_size = stream.resource_info.expected_size as u64;

        let active_stream = active_stream_guard.stream_state.as_mut().unwrap();
        try_join!(
            process_client_stream(
                stream,
                &mut active_stream.tx,
                &active_stream_guard.bytes_received,
                expected_size
            ),
            (&mut active_stream.store_update_fut)
                .map_err(|err| { err.append("Error updating inner store") })
        )?;

        // Close our guard and consider the stream no longer active.
        active_stream_guard.graceful_finish();

        Ok(Response::new(WriteResponse {
            committed_size: expected_size as i64,
        }))
    }

    /// Fast-path write that bypasses channel overhead for stores that support direct Bytes updates.
    /// This buffers all data in memory and calls `update_oneshot` directly.
    async fn inner_write_oneshot(
        &self,
        instance_info: &InstanceInfo,
        digest: DigestInfo,
        mut stream: WriteRequestStreamWrapper<
            impl Stream<Item = Result<WriteRequest, Status>> + Unpin,
        >,
    ) -> Result<Response<WriteResponse>, Error> {
        let expected_size = stream.resource_info.expected_size as u64;

        let mut bytes_received: u64 = 0;
        // Accumulate data. Use Option<Bytes> for the single-chunk fast path
        // (avoids BytesMut allocation + copy when the entire blob arrives in
        // one WriteRequest, which is the common case for small blobs).
        let mut single_chunk: Option<Bytes> = None;
        let mut buffer: Option<BytesMut> = None;

        // Collect all data from client stream
        loop {
            let write_request = match stream.next().await {
                None => {
                    return Err(make_input_err!(
                        "Client closed stream before sending all data"
                    ));
                }
                Some(Err(err)) => return Err(err),
                Some(Ok(write_request)) => write_request,
            };

            if write_request.write_offset < 0 {
                return Err(make_input_err!(
                    "Invalid negative write offset in write request: {}",
                    write_request.write_offset
                ));
            }
            let write_offset = write_request.write_offset as u64;

            // Handle duplicate/resumed data
            let data = if write_offset < bytes_received {
                if (write_offset + write_request.data.len() as u64) < bytes_received {
                    if write_request.finish_write {
                        return Err(make_input_err!(
                            "Resumed stream finished at {} bytes when we already received {} bytes.",
                            write_offset + write_request.data.len() as u64,
                            bytes_received
                        ));
                    }
                    continue;
                }
                write_request
                    .data
                    .slice(usize::try_from(bytes_received - write_offset).unwrap_or(usize::MAX)..)
            } else {
                if write_offset != bytes_received {
                    return Err(make_err!(
                        Code::Unavailable,
                        "Received out of order data (write_offset {} but server has {}). \
                         Partial upload state was lost; retry from committed offset.",
                        write_offset,
                        bytes_received
                    ));
                }
                write_request.data
            };

            if !data.is_empty() {
                bytes_received += data.len() as u64;
                if single_chunk.is_none() && buffer.is_none() {
                    // First chunk — hold zero-copy reference.
                    single_chunk = Some(data);
                } else {
                    // Second+ chunk — spill into BytesMut.
                    let buf = buffer.get_or_insert_with(|| {
                        let capacity = usize::try_from(
                            expected_size.min(64 * 1024 * 1024),
                        )
                        .unwrap_or(64 * 1024 * 1024);
                        let mut b = BytesMut::with_capacity(capacity);
                        if let Some(first) = single_chunk.take() {
                            b.extend_from_slice(&first);
                        }
                        b
                    });
                    buf.extend_from_slice(&data);
                }
            }

            if expected_size < bytes_received {
                return Err(make_input_err!("Received more bytes than expected"));
            }

            if write_request.finish_write {
                // Validate that we received the expected number of bytes
                // before accepting the upload.
                if bytes_received != expected_size {
                    return Err(make_input_err!(
                        "Client declared size {} but only sent {} bytes",
                        expected_size,
                        bytes_received
                    ));
                }
                break;
            }
        }

        // Use the zero-copy single chunk if possible, otherwise the assembled buffer.
        let final_data = if let Some(buf) = buffer {
            buf.freeze()
        } else {
            single_chunk.unwrap_or_default()
        };

        // Clone data for mirroring before store write (Bytes clone is O(1) refcount bump).
        let mirror_data = final_data.clone();

        // Direct update without channel overhead
        let store = instance_info.store.clone();
        store
            .update_oneshot(digest, final_data)
            .await
            .err_tip(|| "Error in update_oneshot")?;

        // Mirror to a random worker using the cloned data — no re-read needed.
        mirror_blob_to_worker(&store, digest, Some(mirror_data));

        // Note: bytes_written_total is updated in the caller (bytestream_write) based on result

        Ok(Response::new(WriteResponse {
            committed_size: expected_size as i64,
        }))
    }

    async fn inner_query_write_status(
        &self,
        query_request: &QueryWriteStatusRequest,
    ) -> Result<Response<QueryWriteStatusResponse>, Error> {
        let mut resource_info = ResourceInfo::new(&query_request.resource_name, true)?;

        let instance = self
            .instance_infos
            .get(resource_info.instance_name.as_ref())
            .err_tip(|| {
                format!(
                    "'instance_name' not configured for '{}'",
                    &resource_info.instance_name
                )
            })?;
        let store_clone = instance.store.clone();

        let digest = DigestInfo::try_new(resource_info.hash.as_ref(), resource_info.expected_size)?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        if let Some(grpc_store) = store_clone.downcast_ref::<GrpcStore>(Some(digest.into())) {
            return grpc_store
                .query_write_status(Request::new(query_request.clone()))
                .await;
        }

        let uuid_str = resource_info
            .uuid
            .take()
            .ok_or_else(|| make_input_err!("UUID must be set if querying write status"))?;
        let uuid_key = parse_uuid_to_key(&uuid_str);

        {
            let active_uploads = instance.active_uploads.lock();
            if let Some((received_bytes, _maybe_idle_stream)) = active_uploads.get(&uuid_key) {
                return Ok(Response::new(QueryWriteStatusResponse {
                    committed_size: received_bytes.load(Ordering::Acquire) as i64,
                    // If we are in the active_uploads map, but the value is None,
                    // it means the stream is not complete.
                    complete: false,
                }));
            }
        }

        let has_fut = store_clone.has(digest);
        let Some(item_size) = has_fut.await.err_tip(|| "Failed to call .has() on store")? else {
            // We lie here and say that the stream needs to start over, even though
            // it was never started. This can happen when the client disconnects
            // before sending the first payload, but the client thinks it did send
            // the payload.
            return Ok(Response::new(QueryWriteStatusResponse {
                committed_size: 0,
                complete: false,
            }));
        };
        Ok(Response::new(QueryWriteStatusResponse {
            committed_size: item_size as i64,
            complete: true,
        }))
    }

    /// Shared write implementation used by both the tonic `write()` handler and
    /// the zero-copy `zero_copy_write()` handler. All preamble (instance lookup,
    /// metrics, GrpcStore shortcut, has-check, oneshot decision) and postamble
    /// (logging, metrics, mirroring) live here so the two entry points are thin
    /// wrappers.
    async fn bytestream_write(
        &self,
        start_time: Instant,
        stream: WriteRequestStreamWrapper<
            impl Stream<Item = Result<WriteRequest, Status>> + Unpin + Send + 'static,
        >,
        zero_copy: bool,
    ) -> Result<Response<WriteResponse>, Error> {
        let instance_name = stream.resource_info.instance_name.as_ref();
        let expected_size = stream.resource_info.expected_size as u64;
        let instance = self
            .instance_infos
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?;

        // Track write request
        instance
            .metrics
            .write_requests_total
            .fetch_add(1, Ordering::Relaxed);

        let store = instance.store.clone();

        let digest = DigestInfo::try_new(
            &stream.resource_info.hash,
            stream.resource_info.expected_size,
        )
        .err_tip(|| "Invalid digest input in ByteStream::write")?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        if let Some(grpc_store) = store.downcast_ref::<GrpcStore>(Some(digest.into())) {
            return grpc_store.write(stream).await.map_err(Into::into);
        }

        // Fast path: skip the write if the blob already exists.
        if store.has(digest).await.unwrap_or(None).is_some() {
            info!(
                %digest,
                size_bytes = expected_size,
                "ByteStream::write: skipped, blob already exists",
            );
            instance
                .metrics
                .write_requests_success
                .fetch_add(1, Ordering::Relaxed);
            return Ok(Response::new(WriteResponse {
                committed_size: expected_size as i64,
            }));
        }

        // Dedup in-flight writes: if another RPC is already writing this
        // exact digest, wait for it instead of writing again.
        let in_flight_tx = {
            let mut guard = instance.in_flight_writes.lock();
            if let Some(rx) = guard.get(&digest) {
                let mut rx = rx.clone();
                drop(guard);
                // Another write is in progress — wait for the result.
                let succeeded = loop {
                    if let Some(ok) = *rx.borrow_and_update() {
                        break ok;
                    }
                    if rx.changed().await.is_err() {
                        break false; // sender dropped = failure
                    }
                };
                if succeeded {
                    info!(
                        %digest,
                        size_bytes = expected_size,
                        "ByteStream::write: coalesced with in-flight write",
                    );
                    instance
                        .metrics
                        .write_requests_success
                        .fetch_add(1, Ordering::Relaxed);
                    return Ok(Response::new(WriteResponse {
                        committed_size: expected_size as i64,
                    }));
                }
                // In-flight write failed — fall through to do our own.
                warn!(
                    %digest,
                    size_bytes = expected_size,
                    "ByteStream::write: in-flight write failed, retrying",
                );
                None
            } else {
                // We're the first writer — create a watch channel.
                let (tx, rx) = tokio::sync::watch::channel(None);
                guard.insert(digest, rx);
                Some(tx)
            }
        };

        let digest_function = stream
            .resource_info
            .digest_function
            .as_deref()
            .map_or_else(
                || Ok(default_digest_hasher_func()),
                DigestHasherFunc::try_from,
            )?;

        // Check if store supports direct oneshot updates (bypasses channel overhead).
        // Use fast-path only when:
        // 1. Store supports oneshot optimization
        // 2. UUID is provided
        // 3. Size is under 64MB (memory safety)
        // 4. This is a NEW upload (UUID not already in active_uploads)
        // 5. The first message has finish_write=true (single-shot upload)
        let use_oneshot = if store.optimized_for(StoreOptimizations::SubscribesToUpdateOneshot)
            && expected_size <= 64 * 1024 * 1024
            && stream.resource_info.uuid.is_some()
        {
            let is_single_shot = stream.is_first_msg_complete();
            if is_single_shot {
                let uuid_str = stream.resource_info.uuid.as_ref().unwrap();
                let uuid_key = parse_uuid_to_key(uuid_str);
                !instance.active_uploads.lock().contains_key(&uuid_key)
            } else {
                false
            }
        } else {
            false
        };

        let oneshot = use_oneshot;
        debug!(
            %digest,
            expected_size,
            oneshot,
            zero_copy,
            "ByteStream::write: starting upload",
        );

        // Build label strings based on zero_copy flag. These must be
        // &'static str for tracing / err_tip messages.
        let (stall_label, tip_label, tip_oneshot_label) = if zero_copy {
            (
                "ByteStream::write(zero-copy)",
                "In ByteStreamServer::write(zero-copy)",
                "In ByteStreamServer::write(zero-copy, oneshot)",
            )
        } else {
            (
                "ByteStream::write",
                "In ByteStreamServer::write",
                "In ByteStreamServer::write (oneshot)",
            )
        };

        let _stall_guard = StallGuard::new(
            nativelink_util::stall_detector::DEFAULT_STALL_THRESHOLD,
            stall_label,
        );
        // Server-side write timeout: abort writes that hang longer than
        // 5 minutes. Prevents stuck operations from holding resources
        // indefinitely (e.g., when a QUIC stream wedges during cache
        // warming bursts).
        const WRITE_TIMEOUT: Duration = Duration::from_secs(300);
        let write_fut = async {
            if use_oneshot {
                self.inner_write_oneshot(instance, digest, stream)
                    .instrument(error_span!("bytestream_write_oneshot", %zero_copy))
                    .with_context(
                        make_ctx_for_hash_func(digest_function)
                            .err_tip(|| tip_label)?,
                    )
                    .await
                    .err_tip(|| tip_oneshot_label)
            } else {
                self.inner_write(instance, digest, stream)
                    .instrument(error_span!("bytestream_write", %zero_copy))
                    .with_context(
                        make_ctx_for_hash_func(digest_function)
                            .err_tip(|| tip_label)?,
                    )
                    .await
                    .err_tip(|| tip_label)
            }
        };
        let result = match tokio::time::timeout(WRITE_TIMEOUT, write_fut).await {
            Ok(r) => r,
            Err(_) => {
                warn!(
                    %digest,
                    expected_size,
                    timeout_secs = WRITE_TIMEOUT.as_secs(),
                    "ByteStream::write: timed out",
                );
                Err(make_err!(
                    Code::DeadlineExceeded,
                    "ByteStream write timed out after {}s for {digest}",
                    WRITE_TIMEOUT.as_secs()
                ))
            }
        };

        // Write finished — signal the result to coalesced waiters BEFORE
        // removing from the map, so new RPCs arriving in between can still
        // find and subscribe to the existing entry.
        if let Some(tx) = in_flight_tx {
            let _ = tx.send(Some(result.is_ok()));
        }
        instance.in_flight_writes.lock().remove(&digest);

        // Track metrics
        #[allow(clippy::cast_possible_truncation)]
        let elapsed_ns = start_time.elapsed().as_nanos() as u64;
        instance
            .metrics
            .write_duration_ns
            .fetch_add(elapsed_ns, Ordering::Relaxed);

        match &result {
            Ok(_) => {
                let elapsed = start_time.elapsed();
                info!(
                    %digest,
                    size_bytes = expected_size,
                    elapsed_ms = elapsed.as_millis() as u64,
                    throughput_mbps = format!("{:.1}", throughput_mbps(expected_size, elapsed)),
                    oneshot,
                    zero_copy,
                    "ByteStream::write: CAS write completed",
                );
                instance
                    .metrics
                    .write_requests_success
                    .fetch_add(1, Ordering::Relaxed);
                instance
                    .metrics
                    .bytes_written_total
                    .fetch_add(expected_size, Ordering::Relaxed);

                // Mirror the blob to a random worker for OOM redundancy.
                // Fire-and-forget: don't delay the Bazel ACK.
                // The oneshot path mirrors inside inner_write_oneshot with
                // the data already in hand. The streaming path must re-read
                // from the store, so we only mirror small blobs (<= 16MB).
                if !use_oneshot && digest.size_bytes() <= MIRROR_STREAM_MAX_SIZE {
                    mirror_blob_to_worker(&store, digest, None);
                }
            }
            Err(e) => {
                error!(
                    %digest,
                    expected_size,
                    elapsed_ms = start_time.elapsed().as_millis() as u64,
                    oneshot,
                    zero_copy,
                    ?e,
                    "ByteStream::write: upload failed",
                );
                instance
                    .metrics
                    .write_requests_failure
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        result
    }

    /// Zero-copy write handler called from `ZeroCopyByteStreamService`.
    ///
    /// Accepts any `Stream<Item = Result<WriteRequest, Status>>` instead of
    /// the tonic-specific `Streaming<WriteRequest>`. The zero-copy stream has
    /// already decoded the gRPC frames without an intermediate copy.
    async fn zero_copy_write(
        &self,
        stream: impl Stream<Item = Result<WriteRequest, Status>> + Send + Unpin + 'static,
        _metadata: &http::HeaderMap,
    ) -> Result<Response<WriteResponse>, Status> {
        let start_time = Instant::now();

        let stream = WriteRequestStreamWrapper::from(stream)
            .await
            .err_tip(|| "Could not unwrap first stream message")
            .map_err(Into::<Status>::into)?;

        self.bytestream_write(start_time, stream, true)
            .await
            .map_err(Into::into)
    }

    /// Handle a ByteStream/Read RPC with zero-copy response encoding.
    ///
    /// This replicates the logic from the tonic `read()` handler but returns a
    /// `ZeroCopyReadBody` that emits the `Bytes` data payload without copying it
    /// through prost's encoder.
    async fn zero_copy_read(
        &self,
        read_request: ReadRequest,
        metadata: &http::HeaderMap,
    ) -> Result<
        http::Response<tonic::body::Body>,
        Status,
    > {
        let start_time = Instant::now();

        let is_worker = metadata.contains_key("x-nativelink-worker");
        let resource_info = ResourceInfo::new(&read_request.resource_name, false)?;
        let instance_name = resource_info.instance_name.as_ref();
        let expected_size = resource_info.expected_size as u64;
        let instance = self
            .instance_infos
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))
            .map_err(Into::<Status>::into)?;

        // Track read request.
        instance
            .metrics
            .read_requests_total
            .fetch_add(1, Ordering::Relaxed);

        let store = instance.store.clone();
        let digest =
            DigestInfo::try_new(resource_info.hash.as_ref(), resource_info.expected_size)
                .map_err(Into::<Status>::into)?;

        // GrpcStore shortcut: proxy the read directly.
        if let Some(grpc_store) = store.downcast_ref::<GrpcStore>(Some(digest.into())) {
            let stream = Box::pin(
                IS_WORKER_REQUEST
                    .scope(is_worker, async {
                        grpc_store
                            .read(Request::new(read_request))
                            .await
                            .map_err(Into::<Status>::into)
                    })
                    .await?,
            );
            let body = ZeroCopyReadBody::new(stream);
            let mut http_response =
                http::Response::new(tonic::body::Body::new(body));
            http_response.headers_mut().insert(
                http::header::CONTENT_TYPE,
                tonic::metadata::GRPC_CONTENT_TYPE,
            );
            return Ok(http_response);
        }

        let digest_function = resource_info
            .digest_function
            .as_deref()
            .map_or_else(|| Ok(default_digest_hasher_func()), DigestHasherFunc::try_from)
            .map_err(Into::<Status>::into)?;

        // Covers stream setup only (inner_read returns a Stream).
        let _stall_guard = StallGuard::new(
            nativelink_util::stall_detector::DEFAULT_STALL_THRESHOLD,
            "ByteStream::zero_copy_read",
        );

        let read_result = self
            .inner_read(instance, digest, read_request, is_worker)
            .instrument(error_span!("bytestream_zero_copy_read"))
            .with_context(
                make_ctx_for_hash_func(digest_function)
                    .err_tip(|| "In ByteStreamServer::zero_copy_read")
                    .map_err(Into::<Status>::into)?,
            )
            .await
            .err_tip(|| "In ByteStreamServer::zero_copy_read");

        // Track metrics.
        #[allow(clippy::cast_possible_truncation)]
        let elapsed_ns = start_time.elapsed().as_nanos() as u64;
        instance
            .metrics
            .read_duration_ns
            .fetch_add(elapsed_ns, Ordering::Relaxed);

        match read_result {
            Ok(stream) => {
                debug!(
                    %digest,
                    size_bytes = expected_size,
                    elapsed_ms = start_time.elapsed().as_millis() as u64,
                    "ByteStream::zero_copy_read: CAS read stream created",
                );
                instance
                    .metrics
                    .read_requests_success
                    .fetch_add(1, Ordering::Relaxed);
                instance
                    .metrics
                    .bytes_read_total
                    .fetch_add(expected_size, Ordering::Relaxed);

                // Wrap in LoggingReadStream to track throughput and log on completion.
                let logging = LoggingReadStream::new(
                    Box::pin(stream),
                    start_time,
                    digest,
                    expected_size,
                );

                let body = ZeroCopyReadBody::new(logging);
                let mut http_response =
                    http::Response::new(tonic::body::Body::new(body));
                http_response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    tonic::metadata::GRPC_CONTENT_TYPE,
                );
                Ok(http_response)
            }
            Err(e) => {
                error!(
                    %digest,
                    size_bytes = expected_size,
                    elapsed_ms = start_time.elapsed().as_millis() as u64,
                    ?e,
                    "ByteStream::zero_copy_read: failed",
                );
                instance
                    .metrics
                    .read_requests_failure
                    .fetch_add(1, Ordering::Relaxed);
                Err(e.into())
            }
        }
    }
}

#[tonic::async_trait]
impl ByteStream for ByteStreamServer {
    type ReadStream = ReadStream;

    #[instrument(
        err,
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn read(
        &self,
        grpc_request: Request<ReadRequest>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        let start_time = Instant::now();

        let is_worker = grpc_request
            .metadata()
            .contains_key("x-nativelink-worker");
        let read_request = grpc_request.into_inner();
        let resource_info = ResourceInfo::new(&read_request.resource_name, false)?;
        let instance_name = resource_info.instance_name.as_ref();
        let expected_size = resource_info.expected_size as u64;
        let instance = self
            .instance_infos
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?;

        // Track read request
        instance
            .metrics
            .read_requests_total
            .fetch_add(1, Ordering::Relaxed);

        let store = instance.store.clone();

        let digest = DigestInfo::try_new(resource_info.hash.as_ref(), resource_info.expected_size)?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        if let Some(grpc_store) = store.downcast_ref::<GrpcStore>(Some(digest.into())) {
            let stream = Box::pin(grpc_store.read(Request::new(read_request)).await?);
            return Ok(Response::new(stream));
        }

        let digest_function = resource_info.digest_function.as_deref().map_or_else(
            || Ok(default_digest_hasher_func()),
            DigestHasherFunc::try_from,
        )?;

        // Covers stream setup only (inner_read returns a Stream).
        // Actual data transfer stalls are not covered by this guard.
        let _stall_guard = StallGuard::new(
            nativelink_util::stall_detector::DEFAULT_STALL_THRESHOLD,
            "ByteStream::read",
        );
        let resp = self
            .inner_read(instance, digest, read_request, is_worker)
            .instrument(error_span!("bytestream_read"))
            .with_context(
                make_ctx_for_hash_func(digest_function).err_tip(|| "In BytestreamServer::read")?,
            )
            .await
            .err_tip(|| "In ByteStreamServer::read")
            .map(|stream| -> Response<Self::ReadStream> {
                // Wrap in LoggingReadStream to log when the client finishes
                // consuming all data (or drops the stream early).
                let logging = LoggingReadStream::new(
                    Box::pin(stream),
                    start_time,
                    digest,
                    expected_size,
                );
                Response::new(Box::pin(logging))
            });

        // Track metrics based on result
        #[allow(clippy::cast_possible_truncation)]
        let elapsed_ns = start_time.elapsed().as_nanos() as u64;
        instance
            .metrics
            .read_duration_ns
            .fetch_add(elapsed_ns, Ordering::Relaxed);

        match &resp {
            Ok(_) => {
                debug!(
                    %digest,
                    size_bytes = expected_size,
                    elapsed_ms = start_time.elapsed().as_millis() as u64,
                    "ByteStream::read: CAS read stream created",
                );
                instance
                    .metrics
                    .read_requests_success
                    .fetch_add(1, Ordering::Relaxed);
                instance
                    .metrics
                    .bytes_read_total
                    .fetch_add(expected_size, Ordering::Relaxed);
            }
            Err(e) => {
                error!(
                    %digest,
                    size_bytes = expected_size,
                    elapsed_ms = start_time.elapsed().as_millis() as u64,
                    ?e,
                    "ByteStream::read: failed",
                );
                instance
                    .metrics
                    .read_requests_failure
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        resp.map_err(Into::into)
    }

    #[instrument(
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn write(
        &self,
        grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        let start_time = Instant::now();

        let request = grpc_request.into_inner();
        let stream = WriteRequestStreamWrapper::from(request)
            .await
            .err_tip(|| "Could not unwrap first stream message")
            .map_err(Into::<Status>::into)?;

        self.bytestream_write(start_time, stream, false)
            .await
            .map_err(Into::into)
    }

    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn query_write_status(
        &self,
        grpc_request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        let request = grpc_request.into_inner();

        // Track query_write_status request - we need to parse the resource name to get the instance
        if let Ok(resource_info) = ResourceInfo::new(&request.resource_name, true) {
            if let Some(instance) = self
                .instance_infos
                .get(resource_info.instance_name.as_ref())
            {
                instance
                    .metrics
                    .query_write_status_total
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        self.inner_query_write_status(&request)
            .await
            .err_tip(|| "Failed on query_write_status() command")
            .map_err(Into::into)
    }
}

/// Tower service wrapper that intercepts ByteStream/Write RPCs and decodes
/// `WriteRequest` messages directly from raw HTTP body frames, eliminating the
/// copy into tonic's `BytesMut` reassembly buffer.
///
/// Read and QueryWriteStatus RPCs pass through to the inner tonic service
/// unchanged.
#[derive(Clone, Debug)]
pub struct ZeroCopyByteStreamService {
    inner: Arc<ByteStreamServer>,
    tonic_service: Server<ByteStreamServer>,
}

impl ZeroCopyByteStreamService {
    /// Apply compression settings to the inner tonic service (for non-Write RPCs).
    pub fn accept_compressed(mut self, encoding: tonic::codec::CompressionEncoding) -> Self {
        self.tonic_service = self.tonic_service.accept_compressed(encoding);
        self
    }

    /// Apply compression settings to the inner tonic service (for non-Write RPCs).
    pub fn send_compressed(mut self, encoding: tonic::codec::CompressionEncoding) -> Self {
        self.tonic_service = self.tonic_service.send_compressed(encoding);
        self
    }
}

impl tonic::server::NamedService for ZeroCopyByteStreamService {
    const NAME: &'static str = "google.bytestream.ByteStream";
}

impl tower::Service<http::Request<tonic::body::Body>> for ZeroCopyByteStreamService {
    type Response = http::Response<tonic::body::Body>;
    type Error = core::convert::Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<tonic::body::Body>) -> Self::Future {
        let path = req.uri().path();

        if path == "/google.bytestream.ByteStream/Write" {
            let inner = self.inner.clone();
            Box::pin(async move {
                let (parts, body) = req.into_parts();
                let metadata = parts.headers;
                let stream = ZeroCopyWriteStream::new(body);

                let result = inner.zero_copy_write(stream, &metadata).await;

                match result {
                    Ok(response) => {
                        let (resp_metadata, write_response, _extensions) = response.into_parts();
                        // Encode the WriteResponse as a gRPC frame.
                        let body_bytes = encode_grpc_unary_response(&write_response);
                        let body = GrpcUnaryBody::new(body_bytes);
                        let mut http_response = http::Response::new(
                            tonic::body::Body::new(body),
                        );
                        *http_response.headers_mut() = resp_metadata.into_headers();
                        http_response.headers_mut().insert(
                            http::header::CONTENT_TYPE,
                            tonic::metadata::GRPC_CONTENT_TYPE,
                        );
                        Ok(http_response)
                    }
                    Err(status) => {
                        Ok(status.into_http())
                    }
                }
            })
        } else if path == "/google.bytestream.ByteStream/Read" {
            let inner = self.inner.clone();
            Box::pin(async move {
                let (parts, body) = req.into_parts();
                let metadata = parts.headers;

                // Decode the unary ReadRequest from the HTTP body.
                let read_request: ReadRequest = match decode_unary_request(body).await {
                    Ok(req) => req,
                    Err(status) => return Ok(status.into_http()),
                };

                match inner.zero_copy_read(read_request, &metadata).await {
                    Ok(http_response) => Ok(http_response),
                    Err(status) => Ok(status.into_http()),
                }
            })
        } else {
            // Delegate QueryWriteStatus to the standard tonic path.
            self.tonic_service.call(req)
        }
    }
}

