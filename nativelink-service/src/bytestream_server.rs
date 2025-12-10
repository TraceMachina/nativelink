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
use core::time::Duration;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::BytesMut;
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
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{
    DigestHasherFunc, default_digest_hasher_func, make_ctx_for_hash_func,
};
use nativelink_util::proto_stream_utils::WriteRequestStreamWrapper;
use nativelink_util::resource_info::ResourceInfo;
use nativelink_util::spawn;
use nativelink_util::store_trait::{Store, StoreLike, StoreOptimizations, UploadSizeInfo};
use nativelink_util::task::JoinHandleDropGuard;
use opentelemetry::context::FutureExt;
use parking_lot::Mutex;
use tokio::time::sleep;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Instrument, Level, debug, error, error_span, info, instrument, trace, warn};

/// If this value changes update the documentation in the config definition.
const DEFAULT_PERSIST_STREAM_ON_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// If this value changes update the documentation in the config definition.
const DEFAULT_MAX_BYTES_PER_STREAM: usize = 64 * 1024;

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
                                info!(
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
        })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
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

        let (uuid, bytes_received, is_collision) =
            match instance.active_uploads.lock().entry(uuid_key) {
                Entry::Occupied(mut entry) => {
                    let maybe_idle_stream = entry.get_mut();
                    if let Some(idle_stream) = maybe_idle_stream.1.take() {
                        // Case 2: Stream exists but is idle, we can resume it
                        let bytes_received = maybe_idle_stream.0.clone();
                        info!(
                            msg = "Joining existing stream",
                            uuid = format!("{:032x}", entry.key())
                        );
                        // Track resumed upload
                        instance
                            .metrics
                            .resumed_uploads
                            .fetch_add(1, Ordering::Relaxed);
                        return idle_stream.into_active_stream(bytes_received, instance);
                    }
                    // Case 3: Stream is active - generate a unique UUID to avoid collision
                    // Using nanosecond timestamp makes collision probability essentially zero
                    let original_key = *entry.key();
                    let unique_key = Self::generate_unique_uuid_key(original_key);
                    warn!(
                        msg = "UUID collision detected, generating unique UUID to prevent conflict",
                        original_uuid = format!("{:032x}", original_key),
                        unique_uuid = format!("{:032x}", unique_key)
                    );
                    // Entry goes out of scope here, releasing the lock

                    let bytes_received = Arc::new(AtomicU64::new(0));
                    let mut active_uploads = instance.active_uploads.lock();
                    // Insert with the unique UUID - this should never collide due to nanosecond precision
                    active_uploads.insert(unique_key, (bytes_received.clone(), None));
                    (unique_key, bytes_received, true)
                }
                Entry::Vacant(entry) => {
                    // Case 1: UUID doesn't exist, create new stream
                    let bytes_received = Arc::new(AtomicU64::new(0));
                    let uuid = *entry.key();
                    // Our stream is "in use" if the key is in the map, but the value is None.
                    entry.insert((bytes_received.clone(), None));
                    (uuid, bytes_received, false)
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

        let (tx, rx) = make_buf_channel_pair();
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
    ) -> Result<impl Stream<Item = Result<ReadResponse, Status>> + Send + use<>, Error> {
        struct ReaderState {
            max_bytes_per_stream: usize,
            rx: DropCloserReadHalf,
            maybe_get_part_result: Option<Result<(), Error>>,
            get_part_fut: Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>,
        }

        let read_limit = u64::try_from(read_request.read_limit)
            .err_tip(|| "Could not convert read_limit to u64")?;

        let (tx, rx) = make_buf_channel_pair();

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
                store
                    .get_part(
                        digest,
                        tx,
                        u64::try_from(read_request.read_offset)
                            .err_tip(|| "Could not convert read_offset to u64")?,
                        read_limit,
                    )
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
                                    trace!(response = ?response);
                                    debug!(response.data = format!("<redacted len({})>", response.data.len()));
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
                                    error!(response = ?e);
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
                        return Err(make_input_err!(
                            "Received out of order data. Got {}, expected {}",
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

        // Pre-allocate buffer for expected size (capped at reasonable limit to prevent DoS)
        let capacity =
            usize::try_from(expected_size.min(64 * 1024 * 1024)).unwrap_or(64 * 1024 * 1024);
        let mut buffer = BytesMut::with_capacity(capacity);
        let mut bytes_received: u64 = 0;

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
                    return Err(make_input_err!(
                        "Received out of order data. Got {}, expected {}",
                        write_offset,
                        bytes_received
                    ));
                }
                write_request.data
            };

            if !data.is_empty() {
                buffer.extend_from_slice(&data);
                bytes_received += data.len() as u64;
            }

            if expected_size < bytes_received {
                return Err(make_input_err!("Received more bytes than expected"));
            }

            if write_request.finish_write {
                break;
            }
        }

        // Direct update without channel overhead
        let store = instance_info.store.clone();
        store
            .update_oneshot(digest, buffer.freeze())
            .await
            .err_tip(|| "Error in update_oneshot")?;

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

        let resp = self
            .inner_read(instance, digest, read_request)
            .instrument(error_span!("bytestream_read"))
            .with_context(
                make_ctx_for_hash_func(digest_function).err_tip(|| "In BytestreamServer::read")?,
            )
            .await
            .err_tip(|| "In ByteStreamServer::read")
            .map(|stream| -> Response<Self::ReadStream> { Response::new(Box::pin(stream)) });

        // Track metrics based on result
        #[allow(clippy::cast_possible_truncation)]
        let elapsed_ns = start_time.elapsed().as_nanos() as u64;
        instance
            .metrics
            .read_duration_ns
            .fetch_add(elapsed_ns, Ordering::Relaxed);

        match &resp {
            Ok(_) => {
                instance
                    .metrics
                    .read_requests_success
                    .fetch_add(1, Ordering::Relaxed);
                instance
                    .metrics
                    .bytes_read_total
                    .fetch_add(expected_size, Ordering::Relaxed);
                debug!(return = "Ok(<stream>)");
            }
            Err(_) => {
                instance
                    .metrics
                    .read_requests_failure
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        resp.map_err(Into::into)
    }

    #[instrument(
        err,
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
            let resp = grpc_store.write(stream).await.map_err(Into::into);
            return resp;
        }

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
        //
        // The oneshot path cannot be used for multi-message streams because:
        // - QueryWriteStatus won't work (no progress tracking)
        // - Resumed streams won't work (no partial progress)
        let use_oneshot = if store.optimized_for(StoreOptimizations::SubscribesToUpdateOneshot)
            && expected_size <= 64 * 1024 * 1024
            && stream.resource_info.uuid.is_some()
        {
            // Check if first message completes the upload (single-shot)
            let is_single_shot = stream.is_first_msg_complete();

            if is_single_shot {
                let uuid_str = stream.resource_info.uuid.as_ref().unwrap();
                let uuid_key = parse_uuid_to_key(uuid_str);
                // Only use oneshot if this UUID is not already being tracked
                !instance.active_uploads.lock().contains_key(&uuid_key)
            } else {
                false
            }
        } else {
            false
        };

        let result = if use_oneshot {
            self.inner_write_oneshot(instance, digest, stream)
                .instrument(error_span!("bytestream_write_oneshot"))
                .with_context(
                    make_ctx_for_hash_func(digest_function)
                        .err_tip(|| "In BytestreamServer::write")?,
                )
                .await
                .err_tip(|| "In ByteStreamServer::write (oneshot)")
        } else {
            self.inner_write(instance, digest, stream)
                .instrument(error_span!("bytestream_write"))
                .with_context(
                    make_ctx_for_hash_func(digest_function)
                        .err_tip(|| "In BytestreamServer::write")?,
                )
                .await
                .err_tip(|| "In ByteStreamServer::write")
        };

        // Track metrics based on result
        #[allow(clippy::cast_possible_truncation)]
        let elapsed_ns = start_time.elapsed().as_nanos() as u64;
        instance
            .metrics
            .write_duration_ns
            .fetch_add(elapsed_ns, Ordering::Relaxed);

        match &result {
            Ok(_) => {
                instance
                    .metrics
                    .write_requests_success
                    .fetch_add(1, Ordering::Relaxed);
                instance
                    .metrics
                    .bytes_written_total
                    .fetch_add(expected_size, Ordering::Relaxed);
            }
            Err(_) => {
                instance
                    .metrics
                    .write_requests_failure
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        result.map_err(Into::into)
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
