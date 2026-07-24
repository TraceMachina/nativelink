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

use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::stream::{FuturesUnordered, unfold};
use futures::{Future, Stream, StreamExt, TryFutureExt, TryStreamExt, future};
use nativelink_config::stores::{GrpcReadBatchingConfig, GrpcSpec, GrpcWriteBatchingConfig};
use nativelink_error::{Error, ResultExt, error_if, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_proto::build::bazel::remote::execution::v2::action_cache_client::ActionCacheClient;
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_client::ContentAddressableStorageClient;
use nativelink_proto::build::bazel::remote::execution::v2::{
    ActionResult, BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, FindMissingBlobsRequest, FindMissingBlobsResponse,
    GetActionResultRequest, GetTreeRequest, GetTreeResponse, SpliceBlobRequest, SpliceBlobResponse,
    SplitBlobRequest, SplitBlobResponse, UpdateActionResultRequest, batch_update_blobs_request,
};
use nativelink_proto::google::bytestream::byte_stream_client::ByteStreamClient;
use nativelink_proto::google::bytestream::{
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::connection_manager::ConnectionManager;
use nativelink_util::digest_hasher::{DigestHasherFunc, default_digest_hasher_func};
use nativelink_util::health_utils::HealthStatusIndicator;
use nativelink_util::proto_stream_utils::{
    FirstStream, WriteRequestStreamWrapper, WriteState, WriteStateWrapper,
};
use nativelink_util::resource_info::ResourceInfo;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{
    RemoveCallback, StoreDriver, StoreKey, StoreOptimizations, UploadSizeInfo,
};
use nativelink_util::telemetry::ClientHeaders;
use nativelink_util::{background_spawn, default_health_status_indicator, tls_utils};
use opentelemetry::context::Context;
use opentelemetry::global;
use opentelemetry::propagation::Injector;
use parking_lot::Mutex;
use prost::Message;
use tokio::sync::{Semaphore, oneshot};
use tokio::time::sleep;
use tonic::metadata::{Ascii, MetadataKey, MetadataValue};
use tonic::{Code, IntoRequest, Request, Response, Status, Streaming};
use tracing::{debug, error, trace, warn};
use uuid::Uuid;

struct TonicMetadataInjector<'a>(&'a mut tonic::metadata::MetadataMap);

impl Injector for TonicMetadataInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(k), Ok(v)) = (
            MetadataKey::from_bytes(key.as_bytes()),
            MetadataValue::try_from(&value),
        ) {
            self.0.insert(k, v);
        }
    }
}

/// Adds configured static headers, forwards nominated client request headers,
/// and injects the current OpenTelemetry trace context into an outgoing gRPC
/// request.
fn enrich_request<T>(
    mut request: Request<T>,
    headers: &[(MetadataKey<Ascii>, MetadataValue<Ascii>)],
    forward_headers: &[String],
) -> Request<T> {
    for (key, value) in headers {
        request.metadata_mut().insert(key.clone(), value.clone());
    }
    if !forward_headers.is_empty()
        && let Some(client_headers) = Context::current().get::<ClientHeaders>()
    {
        for name in forward_headers {
            if let Some(value) = client_headers.0.get(&name.to_lowercase())
                && let (Ok(k), Ok(v)) = (
                    MetadataKey::from_bytes(name.as_bytes()),
                    MetadataValue::try_from(value.as_str()),
                )
            {
                request.metadata_mut().insert(k, v);
            }
        }
    }
    global::get_text_map_propagator(|propagator| {
        propagator.inject(&mut TonicMetadataInjector(request.metadata_mut()));
    });
    request
}

/// Estimated per-entry protobuf and framing overhead charged against a
/// batch's byte budget, so that batches of many tiny blobs cannot push a
/// `BatchReadBlobs`/`BatchUpdateBlobs` message over the gRPC size limit.
const BATCH_PER_ENTRY_OVERHEAD_BYTES: u64 = 256;

/// Number of `BatchUpdateBlobs` RPCs `update_many` dispatches concurrently.
const BATCH_WRITE_DISPATCH_CONCURRENCY: usize = 4;

/// Maximum concurrent per-blob streaming uploads used by `update_many`'s
/// non-batched paths (disabled batching and fallback entries), restoring
/// the concurrency callers previously got from per-file upload streams.
const STREAMING_UPLOAD_CONCURRENCY: usize = 32;

/// A small-blob read waiting to be coalesced into a `BatchReadBlobs` RPC.
#[derive(Debug)]
struct PendingRead {
    digest: DigestInfo,
    digest_function: i32,
    tx: oneshot::Sender<Result<Bytes, Error>>,
}

/// The pending small-blob reads plus the total payload bytes they declare.
#[derive(Debug, Default)]
struct ReadQueue {
    items: VecDeque<PendingRead>,
    bytes: u64,
}

/// State for coalescing small-blob reads into `BatchReadBlobs` RPCs.
///
/// This uses a slot-based group commit scheme: callers enqueue their read and
/// then try to start a detached dispatcher task by acquiring one of
/// `dispatch_slots` semaphore permits. A dispatcher repeatedly drains up to
/// `max_batch_bytes` worth of pending reads into a single `BatchReadBlobs`
/// request until the queue is empty. This is work-conserving (a read never
/// waits while a dispatch slot is free) and uses no timers.
#[derive(Debug, MetricsComponent)]
struct ReadBatcher {
    max_blob_size_bytes: u64,
    max_batch_bytes: u64,
    max_queued_bytes: u64,
    queue: Mutex<ReadQueue>,
    dispatch_slots: Arc<Semaphore>,
    #[metric(help = "Number of BatchReadBlobs RPCs sent by the read coalescer")]
    batches_sent: AtomicU64,
    #[metric(help = "Number of blob reads coalesced into BatchReadBlobs RPCs")]
    blobs_batched: AtomicU64,
    #[metric(
        help = "Number of reads that bypassed batching because the queue byte budget was full"
    )]
    queue_bypasses: AtomicU64,
    #[metric(help = "Number of batched reads that resolved to a per-entry error")]
    batched_read_errors: AtomicU64,
    #[metric(help = "Payload bytes currently waiting in the read coalescer queue")]
    queued_bytes: AtomicU64,
}

impl ReadBatcher {
    fn new(config: &GrpcReadBatchingConfig) -> Self {
        Self {
            max_blob_size_bytes: config.max_blob_size_bytes,
            max_batch_bytes: config.max_batch_bytes,
            max_queued_bytes: config.max_queued_bytes,
            queue: Mutex::new(ReadQueue::default()),
            dispatch_slots: Arc::new(Semaphore::new(config.dispatch_slots)),
            batches_sent: AtomicU64::new(0),
            blobs_batched: AtomicU64::new(0),
            queue_bypasses: AtomicU64::new(0),
            batched_read_errors: AtomicU64::new(0),
            queued_bytes: AtomicU64::new(0),
        }
    }
}

/// Mirrors the default retryable-code classification used by
/// `nativelink_util::retry::Retrier::should_retry`: only codes that are
/// always terminal are considered non-retryable.
const fn is_retryable_code(code: Code) -> bool {
    !matches!(
        code,
        Code::Ok
            | Code::InvalidArgument
            | Code::FailedPrecondition
            | Code::OutOfRange
            | Code::Unimplemented
            | Code::NotFound
            | Code::AlreadyExists
            | Code::PermissionDenied
            | Code::Unauthenticated
    )
}

// This store is usually a pass-through store, but can also be used as a CAS store. Using it as an
// AC store has one major side-effect... The has() function may not give the proper size of the
// underlying data. This might cause issues if embedded in certain stores.
#[derive(Debug, MetricsComponent)]
pub struct GrpcStore {
    #[metric(help = "Instance name for the store")]
    instance_name: String,
    store_type: nativelink_config::stores::StoreType,
    retrier: Retrier,
    connection_manager: ConnectionManager,
    /// Per-RPC timeout. `Duration::ZERO` means disabled.
    rpc_timeout: Duration,
    use_legacy_resource_names: bool,
    headers: Vec<(MetadataKey<Ascii>, MetadataValue<Ascii>)>,
    forward_headers: Vec<String>,
    /// When configured, coalesces small-blob reads into `BatchReadBlobs`
    /// RPCs. `None` means reads always use the `ByteStream` `Read` path.
    #[metric(group = "read_batcher")]
    read_batcher: Option<ReadBatcher>,
    /// When configured, `update_many` packs small blobs into
    /// `BatchUpdateBlobs` RPCs. `None` means every blob uses the
    /// `ByteStream` `Write` path.
    write_batching: Option<GrpcWriteBatchingConfig>,
    /// Used by the read coalescer to hand a strong reference of this store
    /// to detached dispatcher tasks.
    weak_self: Weak<Self>,
}

impl GrpcStore {
    pub async fn new(spec: &GrpcSpec) -> Result<Arc<Self>, Error> {
        Self::new_with_jitter(spec, spec.retry.make_jitter_fn()).await
    }

    pub async fn new_with_jitter(
        spec: &GrpcSpec,
        jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Result<Arc<Self>, Error> {
        error_if!(
            spec.endpoints.is_empty(),
            "Expected at least 1 endpoint in GrpcStore"
        );
        let mut endpoints = Vec::with_capacity(spec.endpoints.len());
        for endpoint_config in &spec.endpoints {
            let endpoint = tls_utils::endpoint(endpoint_config).map_err(|e| {
                Error::from_std_err(Code::InvalidArgument, &e)
                    .append("Invalid URI for GrpcStore endpoint")
            })?;
            endpoints.push(endpoint);
        }

        let rpc_timeout = Duration::from_secs(spec.rpc_timeout_s);

        let read_batcher = match &spec.experimental_read_batching {
            Some(config) => {
                error_if!(
                    config.dispatch_slots == 0,
                    "experimental_read_batching.dispatch_slots must be greater than zero"
                );
                // Batched reads share one upstream RPC across many client
                // requests, so per-client forwarded headers (e.g. credentials)
                // cannot be attached correctly.
                error_if!(
                    !spec.forward_headers.is_empty(),
                    "experimental_read_batching is incompatible with forward_headers"
                );
                Some(ReadBatcher::new(config))
            }
            None => None,
        };

        if let Some(config) = &spec.experimental_write_batching {
            // A blob at the batching threshold must fit in one batch,
            // otherwise an over-budget request bypasses the per-entry
            // streaming fallback and fails the whole call.
            error_if!(
                config
                    .max_blob_size_bytes
                    .saturating_add(BATCH_PER_ENTRY_OVERHEAD_BYTES)
                    > config.max_batch_bytes,
                "experimental_write_batching.max_batch_bytes must be at least max_blob_size_bytes plus the {BATCH_PER_ENTRY_OVERHEAD_BYTES}-byte per-entry overhead"
            );
        }

        let mut headers = Vec::with_capacity(spec.headers.len());
        for (name, value) in &spec.headers {
            // We lowercase keys as HTTP headers are case-insensitive so we should match all cases
            let key = MetadataKey::from_bytes(name.to_lowercase().as_bytes()).map_err(|_| {
                make_err!(Code::InvalidArgument, "Invalid gRPC metadata key: {name}")
            })?;
            let val = MetadataValue::try_from(value.as_str()).map_err(|_| {
                make_err!(
                    Code::InvalidArgument,
                    "Invalid gRPC metadata value for key: {name}"
                )
            })?;
            headers.push((key, val));
        }

        Ok(Arc::new_cyclic(|weak_self| Self {
            weak_self: weak_self.clone(),
            instance_name: spec.instance_name.clone(),
            store_type: spec.store_type,
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn.clone(),
                spec.retry.clone(),
            ),
            connection_manager: ConnectionManager::new(
                endpoints,
                spec.connections_per_endpoint,
                spec.max_concurrent_requests,
                spec.retry.clone(),
                jitter_fn,
            ),
            rpc_timeout,
            use_legacy_resource_names: spec.use_legacy_resource_names,
            read_batcher,
            write_batching: spec.experimental_write_batching,
            headers,
            // We lowercase keys as HTTP headers are case-insensitive so we should match all cases
            forward_headers: spec
                .forward_headers
                .iter()
                .map(|s| s.to_lowercase())
                .collect(),
        }))
    }

    async fn perform_request<F, Fut, R, I>(&self, input: I, mut request: F) -> Result<R, Error>
    where
        F: FnMut(I) -> Fut + Send + Copy,
        Fut: Future<Output = Result<R, Error>> + Send,
        R: Send,
        I: Send + Clone,
    {
        self.retrier
            .retry(unfold(input, move |input| async move {
                let input_clone = input.clone();
                Some((
                    request(input_clone)
                        .await
                        .map_or_else(RetryResult::Retry, RetryResult::Ok),
                    input,
                ))
            }))
            .await
    }

    pub async fn find_missing_blobs(
        &self,
        grpc_request: Request<FindMissingBlobsRequest>,
    ) -> Result<Response<FindMissingBlobsResponse>, Error> {
        error_if!(
            matches!(self.store_type, nativelink_config::stores::StoreType::Ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();

        // Some builds (Chromium for example) do lots of empty requests for some reason, so shortcut them
        if request.blob_digests.is_empty() {
            return Ok(Response::new(FindMissingBlobsResponse {
                missing_blob_digests: vec![],
            }));
        }

        request.instance_name.clone_from(&self.instance_name);
        self.perform_request(request, |request| async move {
            let channel = self
                .connection_manager
                .connection(format!(
                    "find_missing_blobs: ({}) {:?}",
                    request.blob_digests.len(),
                    request.blob_digests
                ))
                .await
                .err_tip(|| "in find_missing_blobs")?;
            ContentAddressableStorageClient::new(channel)
                .find_missing_blobs(enrich_request(
                    Request::new(request),
                    &self.headers,
                    &self.forward_headers,
                ))
                .await
                .err_tip(|| "in GrpcStore::find_missing_blobs")
        })
        .await
    }

    pub async fn batch_update_blobs(
        &self,
        grpc_request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Error> {
        error_if!(
            matches!(self.store_type, nativelink_config::stores::StoreType::Ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();
        request.instance_name.clone_from(&self.instance_name);
        let rpc_timeout = self.rpc_timeout;
        self.perform_request(request, |request| async move {
            let rpc_fut = self
                .connection_manager
                .connection("batch_update_blobs".into())
                .and_then(|channel| async move {
                    ContentAddressableStorageClient::new(channel)
                        .batch_update_blobs(enrich_request(
                            Request::new(request),
                            &self.headers,
                            &self.forward_headers,
                        ))
                        .await
                        .err_tip(|| "in GrpcStore::batch_update_blobs")
                });
            if rpc_timeout > Duration::ZERO {
                tokio::time::timeout(rpc_timeout, rpc_fut)
                    .await
                    .map_err(|_| {
                        make_err!(
                            Code::DeadlineExceeded,
                            "GrpcStore::batch_update_blobs RPC timed out after {}s",
                            rpc_timeout.as_secs()
                        )
                    })?
            } else {
                rpc_fut.await
            }
        })
        .await
    }

    /// Uploads items through the regular streaming path with bounded
    /// concurrency. Used by `update_many` when write batching is disabled
    /// and as the fallback for entries that could not be batched.
    async fn update_items_streaming(
        self: Pin<&Self>,
        items: Vec<(StoreKey<'static>, Bytes)>,
    ) -> Result<(), Error> {
        // Plain loop instead of an iterator closure: async closures over
        // borrowed keys hit HRTB inference errors.
        let mut upload_futures = Vec::with_capacity(items.len());
        for (key, data) in items {
            upload_futures.push(async move { self.update_oneshot(key, data).await });
        }
        let mut uploads =
            futures::stream::iter(upload_futures).buffer_unordered(STREAMING_UPLOAD_CONCURRENCY);
        while let Some(result) = uploads.next().await {
            result.err_tip(|| "In GrpcStore::update_items_streaming")?;
        }
        Ok(())
    }

    pub async fn batch_read_blobs(
        &self,
        grpc_request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Error> {
        error_if!(
            matches!(self.store_type, nativelink_config::stores::StoreType::Ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();
        request.instance_name.clone_from(&self.instance_name);
        self.perform_request(request, |request| async move {
            let channel = self
                .connection_manager
                .connection("batch_read_blobs".into())
                .await
                .err_tip(|| "in batch_read_blobs")?;
            ContentAddressableStorageClient::new(channel)
                .batch_read_blobs(enrich_request(
                    Request::new(request),
                    &self.headers,
                    &self.forward_headers,
                ))
                .await
                .err_tip(|| "in GrpcStore::batch_read_blobs")
        })
        .await
    }

    /// Enqueues a small-blob read for coalescing into a `BatchReadBlobs` RPC
    /// and waits for its result. Returns `None` when the queue is over its
    /// byte budget, in which case the caller must fall back to the
    /// `ByteStream` `Read` path.
    async fn batched_read(
        &self,
        batcher: &ReadBatcher,
        digest: DigestInfo,
    ) -> Option<Result<Bytes, Error>> {
        // Capture the digest function from the caller's ambient context now;
        // the dispatcher runs on a detached task with no such context.
        let digest_function: i32 = Context::current()
            .get::<DigestHasherFunc>()
            .map_or_else(default_digest_hasher_func, |v| *v)
            .proto_digest_func()
            .into();
        let (tx, rx) = oneshot::channel();
        {
            let mut queue = batcher.queue.lock();
            // Admission control. On overflow gracefully degrade to the
            // stream path instead of blocking.
            let new_bytes = queue.bytes.saturating_add(digest.size_bytes());
            if new_bytes > batcher.max_queued_bytes {
                batcher.queue_bypasses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            queue.bytes = new_bytes;
            batcher.queued_bytes.store(new_bytes, Ordering::Relaxed);
            queue.items.push_back(PendingRead {
                digest,
                digest_function,
                tx,
            });
        }

        self.maybe_dispatch_read_batches(batcher);

        Some(rx.await.unwrap_or_else(|_| {
            Err(make_err!(
                Code::Internal,
                "Read batch dispatcher dropped result in GrpcStore::batched_read"
            ))
        }))
    }

    /// Tries to start a read batch dispatcher. This is work-conserving: if a
    /// dispatch slot is free a dispatcher is started immediately, otherwise
    /// one of the active dispatchers is responsible for every currently
    /// queued item. The dispatcher runs as a detached task so that
    /// cancellation of any individual reader can neither abort an in-flight
    /// `BatchReadBlobs` RPC nor strand still-queued waiters.
    fn maybe_dispatch_read_batches(&self, batcher: &ReadBatcher) {
        let Ok(mut permit) = batcher.dispatch_slots.clone().try_acquire_owned() else {
            return;
        };
        let Some(store) = self.weak_self.upgrade() else {
            return;
        };
        background_spawn!("grpc_store_read_batch_dispatch", async move {
            let Some(batcher) = &store.read_batcher else {
                return;
            };
            loop {
                store.dispatch_read_batches(batcher).await;
                drop(permit);
                // Items may have been enqueued between the last drain and
                // the permit release. Re-check so they are not stranded
                // with no active dispatcher.
                if batcher.queue.lock().items.is_empty() {
                    return;
                }
                match batcher.dispatch_slots.clone().try_acquire_owned() {
                    Ok(new_permit) => permit = new_permit,
                    // Another dispatcher is active and will observe these
                    // items (or re-check after releasing its own permit).
                    Err(_) => return,
                }
            }
        });
    }

    /// Drains the pending read queue, sending one `BatchReadBlobs` RPC per
    /// drained batch, until the queue is empty.
    async fn dispatch_read_batches(&self, batcher: &ReadBatcher) {
        loop {
            let batch = {
                let mut queue = batcher.queue.lock();
                let Some(head) = queue.items.front() else {
                    return;
                };
                // All digests in one BatchReadBlobsRequest must use the same
                // digest function. Partition-drain: take items matching the
                // head's digest function from anywhere in the queue (up to
                // the batch budget) and keep the rest in relative order.
                let digest_function = head.digest_function;
                let mut batch = Vec::new();
                let mut batch_bytes = 0u64;
                let mut rest = VecDeque::with_capacity(queue.items.len());
                while let Some(item) = queue.items.pop_front() {
                    let item_cost = item
                        .digest
                        .size_bytes()
                        .saturating_add(BATCH_PER_ENTRY_OVERHEAD_BYTES);
                    if item.digest_function == digest_function
                        && (batch.is_empty()
                            || batch_bytes.saturating_add(item_cost) <= batcher.max_batch_bytes)
                    {
                        batch_bytes = batch_bytes.saturating_add(item_cost);
                        queue.bytes = queue.bytes.saturating_sub(item.digest.size_bytes());
                        batch.push(item);
                    } else {
                        rest.push_back(item);
                    }
                }
                queue.items = rest;
                batcher.queued_bytes.store(queue.bytes, Ordering::Relaxed);
                batch
            };
            self.send_read_batch(batcher, batch).await;
        }
    }

    /// Sends one `BatchReadBlobs` RPC for `batch` and demultiplexes the
    /// per-blob responses back to the waiting readers. One failed item does
    /// not affect its batch-mates; failure of the whole RPC is broadcast to
    /// every item in the batch.
    async fn send_read_batch(&self, batcher: &ReadBatcher, batch: Vec<PendingRead>) {
        let Some(digest_function) = batch.first().map(|item| item.digest_function) else {
            return;
        };
        // Servers may dedupe duplicate digests within one request, so group
        // the waiters per digest and request each digest exactly once,
        // fanning the (refcounted) data out to every waiter.
        let batch_len = u64::try_from(batch.len()).unwrap_or(u64::MAX);
        let mut waiters: HashMap<DigestInfo, Vec<PendingRead>> = HashMap::new();
        for item in batch {
            waiters.entry(item.digest).or_default().push(item);
        }
        let request = BatchReadBlobsRequest {
            // batch_read_blobs() overwrites the instance name, so there is
            // no need to set it here.
            instance_name: String::new(),
            digests: waiters.keys().map(|digest| (*digest).into()).collect(),
            acceptable_compressors: vec![],
            digest_function,
        };
        batcher.batches_sent.fetch_add(1, Ordering::Relaxed);
        batcher
            .blobs_batched
            .fetch_add(batch_len, Ordering::Relaxed);
        let response = match self.batch_read_blobs(Request::new(request)).await {
            Ok(response) => response.into_inner(),
            Err(err) => {
                // The whole RPC failed, so every waiter in this batch gets
                // the error. Waiters may have gone away, ignore send errors.
                for item in waiters.into_values().flatten() {
                    drop(item.tx.send(Err(err.clone())));
                }
                return;
            }
        };
        for entry in response.responses {
            let Some(Ok(entry_digest)) = entry.digest.map(DigestInfo::try_from) else {
                continue;
            };
            let Some(items) = waiters.remove(&entry_digest) else {
                continue;
            };
            let entry_len = u64::try_from(entry.data.len()).unwrap_or(u64::MAX);
            let result = if let Some(status) = entry.status.filter(|status| status.code != 0) {
                Err(Error::from(status)
                    .append("Batch read entry failed in GrpcStore::send_read_batch"))
            } else if entry.compressor != 0 {
                // We requested no acceptable compressors, so data must be
                // returned with the identity compressor.
                Err(make_err!(
                    Code::Internal,
                    "BatchReadBlobs entry for {entry_digest} used unsupported compressor {}",
                    entry.compressor
                ))
            } else if entry_len != entry_digest.size_bytes() {
                Err(make_err!(
                    Code::Internal,
                    "BatchReadBlobs entry for {entry_digest} returned {entry_len} bytes, expected {}",
                    entry_digest.size_bytes()
                ))
            } else {
                Ok(entry.data)
            };
            if result.is_err() {
                batcher.batched_read_errors.fetch_add(
                    u64::try_from(items.len()).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
            }
            for item in items {
                drop(item.tx.send(result.clone()));
            }
        }
        // Any waiter with no matching response entry is missing upstream.
        for item in waiters.into_values().flatten() {
            batcher.batched_read_errors.fetch_add(1, Ordering::Relaxed);
            let err = make_err!(
                Code::NotFound,
                "Blob {} not found in BatchReadBlobs response",
                item.digest
            );
            drop(item.tx.send(Err(err)));
        }
    }

    pub async fn get_tree(
        &self,
        grpc_request: Request<GetTreeRequest>,
    ) -> Result<Response<Streaming<GetTreeResponse>>, Error> {
        error_if!(
            matches!(self.store_type, nativelink_config::stores::StoreType::Ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();
        request.instance_name.clone_from(&self.instance_name);
        self.perform_request(request, |request| async move {
            let channel = self
                .connection_manager
                .connection(format!("get_tree: {:?}", request.root_digest))
                .await
                .err_tip(|| "in get_tree")?;
            ContentAddressableStorageClient::new(channel)
                .get_tree(enrich_request(
                    Request::new(request),
                    &self.headers,
                    &self.forward_headers,
                ))
                .await
                .err_tip(|| "in GrpcStore::get_tree")
        })
        .await
    }

    pub async fn split_blob(
        &self,
        grpc_request: Request<SplitBlobRequest>,
    ) -> Result<Response<SplitBlobResponse>, Error> {
        error_if!(
            matches!(self.store_type, nativelink_config::stores::StoreType::Ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();
        request.instance_name.clone_from(&self.instance_name);
        self.perform_request(request, |request| async move {
            let channel = self
                .connection_manager
                .connection(format!("split_blob: {:?}", request.blob_digest))
                .await
                .err_tip(|| "in split_blob")?;
            ContentAddressableStorageClient::new(channel)
                .split_blob(enrich_request(
                    Request::new(request),
                    &self.headers,
                    &self.forward_headers,
                ))
                .await
                .err_tip(|| "in GrpcStore::split_blob")
        })
        .await
    }

    pub async fn splice_blob(
        &self,
        grpc_request: Request<SpliceBlobRequest>,
    ) -> Result<Response<SpliceBlobResponse>, Error> {
        error_if!(
            matches!(self.store_type, nativelink_config::stores::StoreType::Ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();
        request.instance_name.clone_from(&self.instance_name);
        self.perform_request(request, |request| async move {
            let channel = self
                .connection_manager
                .connection(format!("splice_blob: {:?}", request.blob_digest))
                .await
                .err_tip(|| "in splice_blob")?;
            ContentAddressableStorageClient::new(channel)
                .splice_blob(enrich_request(
                    Request::new(request),
                    &self.headers,
                    &self.forward_headers,
                ))
                .await
                .err_tip(|| "in GrpcStore::splice_blob")
        })
        .await
    }

    fn get_read_request(&self, mut request: ReadRequest) -> Result<ReadRequest, Error> {
        const IS_UPLOAD_FALSE: bool = false;
        let mut resource_info = ResourceInfo::new(&request.resource_name, IS_UPLOAD_FALSE)?;
        if resource_info.instance_name != self.instance_name {
            resource_info.instance_name = Cow::Borrowed(&self.instance_name);
            request.resource_name = resource_info.to_string(IS_UPLOAD_FALSE);
        }
        Ok(request)
    }

    async fn read_internal(
        &self,
        request: ReadRequest,
    ) -> Result<impl Stream<Item = Result<ReadResponse, Status>> + use<>, Error> {
        let channel = self
            .connection_manager
            .connection(format!("read_internal: {}", request.resource_name))
            .await
            .err_tip(|| "in read_internal")?;
        let mut response = ByteStreamClient::new(channel)
            .read(enrich_request(
                Request::new(request),
                &self.headers,
                &self.forward_headers,
            ))
            .await
            .err_tip(|| "in GrpcStore::read")?
            .into_inner();
        let first_response = response
            .message()
            .await
            .err_tip(|| "Fetching first chunk in GrpcStore::read()")?;
        Ok(FirstStream::new(first_response, response))
    }

    pub async fn read<R>(
        &self,
        grpc_request: R,
    ) -> Result<impl Stream<Item = Result<ReadResponse, Status>> + use<R>, Error>
    where
        R: IntoRequest<ReadRequest>,
    {
        error_if!(
            matches!(self.store_type, nativelink_config::stores::StoreType::Ac),
            "CAS operation on AC store"
        );

        let request = self.get_read_request(grpc_request.into_request().into_inner())?;
        self.perform_request(request, |request| async move {
            self.read_internal(request).await
        })
        .await
    }

    pub async fn write<T, E>(
        &self,
        stream: WriteRequestStreamWrapper<T>,
    ) -> Result<Response<WriteResponse>, Error>
    where
        T: Stream<Item = Result<WriteRequest, E>> + Unpin + Send + 'static,
        E: Into<Error> + 'static,
    {
        error_if!(
            matches!(self.store_type, nativelink_config::stores::StoreType::Ac),
            "CAS operation on AC store"
        );

        let local_state = Arc::new(Mutex::new(WriteState::new(
            self.instance_name.clone(),
            stream,
        )));

        let write_start = std::time::Instant::now();
        let instance_name = self.instance_name.clone();
        let rpc_timeout = self.rpc_timeout;
        trace!(
            instance_name = %instance_name,
            rpc_timeout_s = rpc_timeout.as_secs(),
            "GrpcStore::write: starting ByteStream write",
        );
        let mut attempt: u32 = 0;
        let result = self
            .retrier
            .retry(unfold(local_state, move |local_state| {
                attempt += 1;
                let instance_name = instance_name.clone();
                async move {
                    // The client write may occur on a separate thread and
                    // therefore in order to share the state with it we have to
                    // wrap it in a Mutex and retrieve it after the write
                    // has completed.  There is no way to get the value back
                    // from the client.
                    trace!(
                        instance_name = %instance_name,
                        attempt,
                        "GrpcStore::write: requesting connection from pool",
                    );
                    let conn_start = std::time::Instant::now();
                    let rpc_fut = self.connection_manager.connection("write".into()).and_then(
                        |channel| {
                            let conn_elapsed = conn_start.elapsed();
                            let instance_for_rpc = instance_name.clone();
                            let conn_elapsed_ms =
                                u64::try_from(conn_elapsed.as_millis()).unwrap_or(u64::MAX);
                            trace!(
                                instance_name = %instance_for_rpc,
                                conn_elapsed_ms,
                                "GrpcStore::write: got connection, starting ByteStream.Write RPC",
                            );
                            let rpc_start = std::time::Instant::now();
                            let local_state_for_rpc = local_state.clone();
                            async move {
                                let res = ByteStreamClient::new(channel)
                                    .write(enrich_request(
                                        Request::new(WriteStateWrapper::new(local_state_for_rpc)),
                                        &self.headers,
                                        &self.forward_headers,
                                    ))
                                    .await
                                    .err_tip(|| "in GrpcStore::write");
                                let rpc_elapsed_ms = u64::try_from(rpc_start.elapsed().as_millis())
                                    .unwrap_or(u64::MAX);
                                trace!(
                                    instance_name = %instance_for_rpc,
                                    rpc_elapsed_ms,
                                    success = res.is_ok(),
                                    "GrpcStore::write: ByteStream.Write RPC returned",
                                );
                                res
                            }
                        },
                    );

                    let result = if rpc_timeout > Duration::ZERO {
                        match tokio::time::timeout(rpc_timeout, rpc_fut).await {
                            Ok(res) => res,
                            Err(_elapsed) => {
                                warn!(
                                    instance_name = %instance_name,
                                    attempt,
                                    rpc_timeout_s = rpc_timeout.as_secs(),
                                    "GrpcStore::write: per-RPC timeout exceeded, cancelling",
                                );
                                #[allow(unused_qualifications)]
                                Err(nativelink_error::make_err!(
                                    nativelink_error::Code::DeadlineExceeded,
                                    "GrpcStore::write RPC timed out after {}s",
                                    rpc_timeout.as_secs()
                                ))
                            }
                        }
                    } else {
                        rpc_fut.await
                    };

                    // Get the state back from StateWrapper, this should be
                    // uncontended since write has returned.
                    let mut local_state_locked = local_state.lock();

                    let result = local_state_locked
                        .take_read_stream_error()
                        .map(|err| RetryResult::Err(err.append("Where read_stream_error was set")))
                        .unwrap_or_else(|| {
                            // No stream error, handle the original result
                            match result {
                                Ok(response) => RetryResult::Ok(response),
                                Err(ref err) => {
                                    warn!(
                                        instance_name = %instance_name,
                                        attempt,
                                        ?err,
                                        can_resume = local_state_locked.can_resume(),
                                        "GrpcStore::write: RPC failed",
                                    );
                                    if local_state_locked.can_resume() {
                                        local_state_locked.resume();
                                        RetryResult::Retry(err.clone())
                                    } else {
                                        RetryResult::Err(
                                            err.clone().append("Retry is not possible"),
                                        )
                                    }
                                }
                            }
                        });

                    drop(local_state_locked);
                    Some((result, local_state))
                }
            }))
            .await?;

        let total_elapsed_ms = u64::try_from(write_start.elapsed().as_millis()).unwrap_or(u64::MAX);
        trace!(
            instance_name = %self.instance_name,
            total_elapsed_ms,
            "GrpcStore::write: completed successfully",
        );
        Ok(result)
    }

    pub async fn query_write_status(
        &self,
        grpc_request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Error> {
        const IS_UPLOAD_TRUE: bool = true;

        error_if!(
            matches!(self.store_type, nativelink_config::stores::StoreType::Ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();

        let mut request_info = ResourceInfo::new(&request.resource_name, IS_UPLOAD_TRUE)?;
        if request_info.instance_name != self.instance_name {
            request_info.instance_name = Cow::Borrowed(&self.instance_name);
            request.resource_name = request_info.to_string(IS_UPLOAD_TRUE);
        }

        self.perform_request(request, |request| async move {
            let channel = self
                .connection_manager
                .connection(format!("query_write_status: {}", request.resource_name))
                .await
                .err_tip(|| "in query_write_status")?;
            ByteStreamClient::new(channel)
                .query_write_status(enrich_request(
                    Request::new(request),
                    &self.headers,
                    &self.forward_headers,
                ))
                .await
                .err_tip(|| "in GrpcStore::query_write_status")
        })
        .await
    }

    pub async fn get_action_result(
        &self,
        grpc_request: Request<GetActionResultRequest>,
    ) -> Result<Response<ActionResult>, Error> {
        let mut request = grpc_request.into_inner();
        request.instance_name.clone_from(&self.instance_name);
        self.perform_request(request, |request| async move {
            let channel = self
                .connection_manager
                .connection(format!("get_action_result: {:?}", request.action_digest))
                .await
                .err_tip(|| "in get_action_result")?;
            ActionCacheClient::new(channel)
                .get_action_result(enrich_request(
                    Request::new(request),
                    &self.headers,
                    &self.forward_headers,
                ))
                .await
                .err_tip(|| "in GrpcStore::get_action_result")
        })
        .await
    }

    pub async fn update_action_result(
        &self,
        grpc_request: Request<UpdateActionResultRequest>,
    ) -> Result<Response<ActionResult>, Error> {
        let mut request = grpc_request.into_inner();
        request.instance_name.clone_from(&self.instance_name);
        self.perform_request(request, |request| async move {
            let channel = self
                .connection_manager
                .connection(format!("update_action_result: {:?}", request.action_digest))
                .await
                .err_tip(|| "in update_action_result")?;
            ActionCacheClient::new(channel)
                .update_action_result(enrich_request(
                    Request::new(request),
                    &self.headers,
                    &self.forward_headers,
                ))
                .await
                .err_tip(|| "in GrpcStore::update_action_result")
        })
        .await
    }

    async fn get_action_result_from_digest(
        &self,
        digest: DigestInfo,
    ) -> Result<Response<ActionResult>, Error> {
        let action_result_request = GetActionResultRequest {
            instance_name: self.instance_name.clone(),
            action_digest: Some(digest.into()),
            inline_stdout: false,
            inline_stderr: false,
            inline_output_files: Vec::new(),
            digest_function: Context::current()
                .get::<DigestHasherFunc>()
                .map_or_else(default_digest_hasher_func, |v| *v)
                .proto_digest_func()
                .into(),
        };
        self.get_action_result(Request::new(action_result_request))
            .await
    }

    async fn get_action_result_as_part(
        &self,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let action_result = self
            .get_action_result_from_digest(digest)
            .await
            .map(Response::into_inner)
            .err_tip(|| "Action result not found")?;
        // TODO: Would be better to avoid all the encoding and decoding in this
        //       file, however there's no way to currently get raw bytes from a
        //       generated prost request unfortunately.
        let mut value = BytesMut::new();
        action_result
            .encode(&mut value)
            .err_tip(|| "Could not encode upstream action result")?;

        let default_len = value.len() - offset;
        let length = length.unwrap_or(default_len).min(default_len);
        if length > 0 {
            writer
                .send(value.freeze().slice(offset..offset + length))
                .await
                .err_tip(|| "Failed to write data in grpc store")?;
        }
        writer
            .send_eof()
            .err_tip(|| "Failed to write EOF in grpc store get_action_result_as_part")?;
        Ok(())
    }

    async fn update_action_result_from_bytes(
        &self,
        digest: DigestInfo,
        mut reader: DropCloserReadHalf,
    ) -> Result<u64, Error> {
        let bytes = reader.consume(None).await?;
        let len = bytes.len() as u64;
        let action_result = ActionResult::decode(bytes)
            .err_tip(|| "Failed to decode ActionResult in update_action_result_from_bytes")?;
        let update_action_request = UpdateActionResultRequest {
            instance_name: self.instance_name.clone(),
            action_digest: Some(digest.into()),
            action_result: Some(action_result),
            results_cache_policy: None,
            digest_function: Context::current()
                .get::<DigestHasherFunc>()
                .map_or_else(default_digest_hasher_func, |v| *v)
                .proto_digest_func()
                .into(),
        };
        self.update_action_result(Request::new(update_action_request))
            .await
            .map(|_| len)
    }
}

#[async_trait]
impl StoreDriver for GrpcStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        optimization == StoreOptimizations::SubscribesToUpdateMany
            && self.write_batching.is_some()
            && !matches!(self.store_type, nativelink_config::stores::StoreType::Ac)
    }

    async fn update_many(
        self: Pin<&Self>,
        items: Vec<(StoreKey<'static>, Bytes)>,
    ) -> Result<(), Error> {
        let Some(config) = self.write_batching else {
            return self.update_items_streaming(items).await;
        };
        if matches!(self.store_type, nativelink_config::stores::StoreType::Ac) {
            return self.update_items_streaming(items).await;
        }

        // Partition into batchable small blobs and streaming fallbacks.
        // Duplicate digests are uploaded once: CAS content is identical by
        // definition and servers may reject duplicates within one request.
        let mut seen_digests: HashSet<DigestInfo> = HashSet::with_capacity(items.len());
        let mut batchable: Vec<(DigestInfo, Bytes)> = Vec::with_capacity(items.len());
        let mut fallback: Vec<(StoreKey<'static>, Bytes)> = Vec::new();
        for (key, data) in items {
            let digest = key.borrow().into_digest();
            if data.len() as u64 > config.max_blob_size_bytes {
                fallback.push((key, data));
            } else if seen_digests.insert(digest) {
                batchable.push((digest, data));
            }
        }

        let digest_function: i32 = Context::current()
            .get::<DigestHasherFunc>()
            .map_or_else(default_digest_hasher_func, |v| *v)
            .proto_digest_func()
            .into();

        // Pack under the batch byte budget.
        let mut chunks: Vec<Vec<(DigestInfo, Bytes)>> = Vec::new();
        let mut chunk: Vec<(DigestInfo, Bytes)> = Vec::new();
        let mut chunk_bytes = 0u64;
        for (digest, data) in batchable {
            let entry_cost = data.len() as u64 + BATCH_PER_ENTRY_OVERHEAD_BYTES;
            if !chunk.is_empty() && chunk_bytes + entry_cost > config.max_batch_bytes {
                chunks.push(core::mem::take(&mut chunk));
                chunk_bytes = 0;
            }
            chunk.push((digest, data));
            chunk_bytes += entry_cost;
        }
        if !chunk.is_empty() {
            chunks.push(chunk);
        }

        // Dispatch chunk RPCs concurrently; each task returns the items of
        // that chunk that must fall back to the streaming path.
        let mut chunk_tasks = futures::stream::iter(chunks.into_iter().map(|chunk| async move {
            let request = BatchUpdateBlobsRequest {
                // batch_update_blobs() overwrites the instance name.
                instance_name: String::new(),
                requests: chunk
                    .iter()
                    .map(|(digest, data)| batch_update_blobs_request::Request {
                        digest: Some((*digest).into()),
                        data: data.clone(),
                        compressor: 0,
                    })
                    .collect(),
                digest_function,
            };
            let response = match self.batch_update_blobs(Request::new(request)).await {
                Ok(response) => response.into_inner(),
                // A whole-RPC failure (already through the retrier) falls
                // back to per-blob streams: an intermediary's message-size
                // limit can reject the batch while individual streams
                // would succeed.
                Err(err) => {
                    debug!(
                        ?err,
                        "BatchUpdateBlobs failed as a whole; falling back to streaming uploads for this chunk",
                    );
                    return Ok(chunk
                        .into_iter()
                        .map(|(digest, data)| (StoreKey::from(digest), data))
                        .collect::<Vec<_>>());
                }
            };

            enum BatchEntryStatus {
                Success,
                Error(Error),
                Malformed,
            }

            let mut status_by_digest: HashMap<DigestInfo, BatchEntryStatus> =
                HashMap::with_capacity(response.responses.len());
            for entry in response.responses {
                let Some(Ok(digest)) = entry.digest.map(DigestInfo::try_from) else {
                    continue;
                };
                let entry_status = match entry.status {
                    None => BatchEntryStatus::Malformed,
                    Some(status) if !(0..=16).contains(&status.code) => {
                        BatchEntryStatus::Malformed
                    }
                    Some(status) if status.code == Code::Ok as i32 => BatchEntryStatus::Success,
                    Some(status) => BatchEntryStatus::Error(Error::from(status)),
                };

                // A malformed or non-success duplicate must not be hidden by
                // a later success response for the same digest.
                if let Some(existing) = status_by_digest.get_mut(&digest) {
                    let replace = match existing {
                        BatchEntryStatus::Malformed => false,
                        BatchEntryStatus::Error(_) => {
                            matches!(entry_status, BatchEntryStatus::Malformed)
                        }
                        BatchEntryStatus::Success => {
                            !matches!(entry_status, BatchEntryStatus::Success)
                        }
                    };
                    if replace {
                        *existing = entry_status;
                    }
                } else {
                    status_by_digest.insert(digest, entry_status);
                }
            }
            let mut chunk_fallback = Vec::new();
            for (digest, data) in chunk {
                match status_by_digest.remove(&digest) {
                    // Entry succeeded.
                    Some(BatchEntryStatus::Success) => {}
                    Some(BatchEntryStatus::Error(err)) if is_retryable_code(err.code) => {
                        trace!(
                            ?digest,
                            ?err,
                            "Batched upload entry failed with retryable error, falling back to ByteStream write",
                        );
                        chunk_fallback.push((digest.into(), data));
                    }
                    Some(BatchEntryStatus::Error(err)) => {
                        return Err(err.append(format!(
                            "in BatchUpdateBlobs response for {digest} in GrpcStore::update_many"
                        )));
                    }
                    Some(BatchEntryStatus::Malformed) => {
                        trace!(
                            ?digest,
                            "Batched upload entry had a missing or malformed status, falling back to ByteStream write",
                        );
                        chunk_fallback.push((digest.into(), data));
                    }
                    // Server omitted the entry; fall back to the streaming
                    // path rather than guessing at its state.
                    None => chunk_fallback.push((digest.into(), data)),
                }
            }
            Ok::<_, Error>(chunk_fallback)
        }))
        .buffer_unordered(BATCH_WRITE_DISPATCH_CONCURRENCY);
        while let Some(chunk_fallback) = chunk_tasks.next().await {
            fallback.extend(chunk_fallback?);
        }
        drop(chunk_tasks);

        self.update_items_streaming(fallback).await
    }

    // NOTE: This function can only be safely used on CAS stores. AC stores may return a size that
    // is incorrect.
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        if matches!(self.store_type, nativelink_config::stores::StoreType::Ac) {
            keys.iter()
                .zip(results.iter_mut())
                .map(|(key, result)| async move {
                    // The length of an AC is incorrect, so we don't figure out the
                    // length, instead the biggest possible result is returned in the
                    // hope that we detect incorrect usage.
                    self.get_action_result_from_digest(key.borrow().into_digest())
                        .await?;
                    *result = Some(u64::MAX);
                    Ok::<_, Error>(())
                })
                .collect::<FuturesUnordered<_>>()
                .try_for_each(|()| future::ready(Ok(())))
                .await
                .err_tip(|| "Getting upstream action cache entry")?;
            return Ok(());
        }

        let missing_blobs_response = self
            .find_missing_blobs(Request::new(FindMissingBlobsRequest {
                instance_name: self.instance_name.clone(),
                blob_digests: keys
                    .iter()
                    .map(|k| k.borrow().into_digest().into())
                    .collect(),
                digest_function: Context::current()
                    .get::<DigestHasherFunc>()
                    .map_or_else(default_digest_hasher_func, |v| *v)
                    .proto_digest_func()
                    .into(),
            }))
            .await?
            .into_inner();

        // Since the ordering is not guaranteed above, the matching has to check
        // all missing blobs against all entries in the unsorted digest list.
        // To optimise this, the missing digests are sorted and then it is
        // efficient to perform a binary search for each digest within the
        // missing list.
        let mut missing_digests =
            Vec::with_capacity(missing_blobs_response.missing_blob_digests.len());
        for missing_digest in missing_blobs_response.missing_blob_digests {
            missing_digests.push(DigestInfo::try_from(missing_digest)?);
        }
        missing_digests.sort_unstable();
        for (digest, result) in keys
            .iter()
            .map(|v| v.borrow().into_digest())
            .zip(results.iter_mut())
        {
            match missing_digests.binary_search(&digest) {
                Ok(_) => *result = None,
                Err(_) => *result = Some(digest.size_bytes()),
            }
        }

        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        struct LocalState {
            resource_name: String,
            reader: DropCloserReadHalf,
            did_error: bool,
            bytes_received: i64,
        }

        let digest = key.into_digest();
        if matches!(self.store_type, nativelink_config::stores::StoreType::Ac) {
            return self.update_action_result_from_bytes(digest, reader).await;
        }

        let mut buf = Uuid::encode_buffer();
        let resource_name = if self.use_legacy_resource_names {
            format!(
                "{}/uploads/{}/blobs/{}/{}",
                &self.instance_name,
                Uuid::new_v4().hyphenated().encode_lower(&mut buf),
                digest.packed_hash(),
                digest.size_bytes(),
            )
        } else {
            let digest_function = Context::current()
                .get::<DigestHasherFunc>()
                .map_or_else(default_digest_hasher_func, |v| *v)
                .proto_digest_func()
                .as_str_name()
                .to_ascii_lowercase();
            format!(
                "{}/uploads/{}/blobs/{}/{}/{}",
                &self.instance_name,
                Uuid::new_v4().hyphenated().encode_lower(&mut buf),
                digest_function,
                digest.packed_hash(),
                digest.size_bytes(),
            )
        };
        trace!(
            resource_name = %resource_name,
            digest_hash = %digest.packed_hash(),
            digest_size = digest.size_bytes(),
            "GrpcStore::update: starting upload for digest",
        );
        let local_state = LocalState {
            resource_name,
            reader,
            did_error: false,
            bytes_received: 0,
        };

        let stream = Box::pin(unfold(local_state, |mut local_state| async move {
            if local_state.did_error {
                error!("GrpcStore::update() polled stream after error was returned");
                return None;
            }
            let data = match local_state
                .reader
                .recv()
                .await
                .err_tip(|| "In GrpcStore::update()")
            {
                Ok(data) => data,
                Err(err) => {
                    local_state.did_error = true;
                    return Some((Err(err), local_state));
                }
            };

            let write_offset = local_state.bytes_received;
            local_state.bytes_received += data.len() as i64;

            Some((
                Ok(WriteRequest {
                    resource_name: local_state.resource_name.clone(),
                    write_offset,
                    finish_write: data.is_empty(), // EOF is when no data was polled.
                    data,
                }),
                local_state,
            ))
        }));

        self.write(
            WriteRequestStreamWrapper::from(stream)
                .await
                .err_tip(|| "in GrpcStore::update()")?,
        )
        .await
        .err_tip(|| "in GrpcStore::update()")?;

        Ok(digest.size_bytes())
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        struct LocalState<'a> {
            resource_name: String,
            writer: &'a mut DropCloserWriteHalf,
            read_offset: i64,
            read_limit: i64,
        }

        let is_digest_key = matches!(key, StoreKey::Digest(_));
        let digest = key.into_digest();
        if matches!(self.store_type, nativelink_config::stores::StoreType::Ac) {
            let offset = usize::try_from(offset).err_tip(|| "Could not convert offset to usize")?;
            let length = length
                .map(|v| usize::try_from(v).err_tip(|| "Could not convert length to usize"))
                .transpose()?;

            return self
                .get_action_result_as_part(digest, writer, offset, length)
                .await;
        }

        // Shortcut for empty blobs.
        if digest.size_bytes() == 0 {
            return writer.send_eof();
        }

        // When configured, coalesce full reads of small blobs into
        // BatchReadBlobs RPCs. `batched_read` returns `None` when the queue
        // is over budget, in which case we fall through to the stream path.
        if let Some(batcher) = &self.read_batcher
            && is_digest_key
            && offset == 0
            && length.is_none_or(|len| len >= digest.size_bytes())
            && digest.size_bytes() <= batcher.max_blob_size_bytes
            && let Some(result) = self.batched_read(batcher, digest).await
        {
            match result {
                Ok(data) => {
                    if !data.is_empty() {
                        writer
                            .send(data)
                            .await
                            .err_tip(|| "Failed to write data in GrpcStore::get_part()")?;
                    }
                    return writer
                        .send_eof()
                        .err_tip(|| "Failed to send EOF in GrpcStore::get_part()");
                }
                // A retryable error falls through to the ByteStream path
                // below, which re-enters the full retry machinery. This
                // matches the retry behavior reads had before batching.
                Err(err) if is_retryable_code(err.code) => {
                    warn!(
                        ?err,
                        "Batched read failed with retryable error, falling back to ByteStream read",
                    );
                }
                Err(err) => return Err(err.append("in GrpcStore::get_part()")),
            }
        }

        let resource_name = if self.use_legacy_resource_names {
            format!(
                "{}/blobs/{}/{}",
                &self.instance_name,
                digest.packed_hash(),
                digest.size_bytes(),
            )
        } else {
            let digest_function = Context::current()
                .get::<DigestHasherFunc>()
                .map_or_else(default_digest_hasher_func, |v| *v)
                .proto_digest_func()
                .as_str_name()
                .to_ascii_lowercase();
            format!(
                "{}/blobs/{}/{}/{}",
                &self.instance_name,
                digest_function,
                digest.packed_hash(),
                digest.size_bytes(),
            )
        };

        let local_state = LocalState {
            resource_name,
            writer,
            read_offset: i64::try_from(offset).err_tip(|| "Could not convert offset to i64")?,
            read_limit: i64::try_from(length.unwrap_or(0))
                .err_tip(|| "Could not convert length to i64")?,
        };

        self.retrier
            .retry(unfold(local_state, move |mut local_state| async move {
                let request = ReadRequest {
                    resource_name: local_state.resource_name.clone(),
                    read_offset: local_state.read_offset,
                    read_limit: local_state.read_limit,
                };
                let mut stream = match self
                    .read_internal(request)
                    .await
                    .err_tip(|| "in GrpcStore::get_part()")
                {
                    Ok(stream) => stream,
                    Err(err) => return Some((RetryResult::Retry(err), local_state)),
                };

                loop {
                    let data = match stream.next().await {
                        // Create an empty response to represent EOF.
                        None => Bytes::new(),
                        Some(Ok(message)) => message.data,
                        Some(Err(status)) => {
                            return Some((
                                RetryResult::Retry(
                                    Into::<Error>::into(status)
                                        .append("While fetching message in GrpcStore::get_part()"),
                                ),
                                local_state,
                            ));
                        }
                    };
                    let length = data.len() as i64;
                    // This is the usual exit from the loop at EOF.
                    if length == 0 {
                        let eof_result = local_state
                            .writer
                            .send_eof()
                            .err_tip(|| "Could not send eof in GrpcStore::get_part()")
                            .map_or_else(RetryResult::Err, RetryResult::Ok);
                        return Some((eof_result, local_state));
                    }
                    // Forward the data upstream.
                    if let Err(err) = local_state
                        .writer
                        .send(data)
                        .await
                        .err_tip(|| "While sending in GrpcStore::get_part()")
                    {
                        return Some((RetryResult::Err(err), local_state));
                    }
                    local_state.read_offset += length;
                }
            }))
            .await
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(self: Arc<Self>, _callback: RemoveCallback) -> Result<(), Error> {
        Err(Error::new(
            Code::Internal,
            "gRPC stores are incompatible with removal callbacks".to_string(),
        ))
    }
}

default_health_status_indicator!(GrpcStore);
