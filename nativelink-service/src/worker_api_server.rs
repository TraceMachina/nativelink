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
use core::pin::Pin;
use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use futures::stream::unfold;
use futures::{Stream, StreamExt};
use nativelink_config::cas_server::WorkerApiConfig;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_scheduler::Update;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::worker_api_server::{
    WorkerApi, WorkerApiServer as Server,
};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_worker;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    execute_result, ExecuteComplete, ExecuteResult, GoingAwayRequest, KeepAliveRequest,
    UpdateForScheduler, UpdateForWorker, UploadMissingBlobsRequest,
};
use nativelink_util::blob_locality_map::SharedBlobLocalityMap;
use nativelink_util::common::DigestInfo;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::background_spawn;
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::operation_state_manager::UpdateOperationType;
use nativelink_util::platform_properties::PlatformProperties;
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use rand::RngCore;
use tokio::sync::mpsc;
use tokio::time::interval;
use tonic::{Response, Status};
use tracing::{debug, error, info, warn, instrument, Level};
use uuid::Uuid;

use nativelink_proto::build::bazel::remote::execution::v2::Digest;

pub type ConnectWorkerStream =
    Pin<Box<dyn Stream<Item = Result<UpdateForWorker, Status>> + Send + Sync + 'static>>;

pub type NowFn = Box<dyn Fn() -> Result<Duration, Error> + Send + Sync>;

pub struct WorkerApiServer {
    scheduler: Arc<dyn WorkerScheduler>,
    now_fn: Arc<NowFn>,
    node_id: [u8; 6],
    locality_map: Option<SharedBlobLocalityMap>,
    /// CAS store for checking blob existence during backfill requests.
    cas_store: Option<Store>,
}

impl core::fmt::Debug for WorkerApiServer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WorkerApiServer")
            .field("node_id", &self.node_id)
            .finish_non_exhaustive()
    }
}

impl WorkerApiServer {
    pub fn new(
        config: &WorkerApiConfig,
        schedulers: &HashMap<String, Arc<dyn WorkerScheduler>>,
        locality_map: Option<SharedBlobLocalityMap>,
        cas_store: Option<Store>,
    ) -> Result<Self, Error> {
        let node_id = {
            let mut out = [0; 6];
            rand::rng().fill_bytes(&mut out);
            out
        };
        for scheduler in schedulers.values() {
            // This will protect us from holding a reference to the scheduler forever in the
            // event our ExecutionServer dies. Our scheduler is a weak ref, so the spawn will
            // eventually see the Arc went away and return.
            let weak_scheduler = Arc::downgrade(scheduler);
            background_spawn!("worker_api_server", async move {
                let mut ticker = interval(Duration::from_secs(1));
                loop {
                    ticker.tick().await;
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("Error: system time is now behind unix epoch");
                    match weak_scheduler.upgrade() {
                        Some(scheduler) => {
                            if let Err(err) =
                                scheduler.remove_timedout_workers(timestamp.as_secs()).await
                            {
                                error!(?err, "Failed to remove_timedout_workers",);
                            }
                        }
                        // If we fail to upgrade, our service is probably destroyed, so return.
                        None => return,
                    }
                }
            });
        }

        Self::new_with_now_fn(
            config,
            schedulers,
            Box::new(move || {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| make_err!(Code::Internal, "System time is now behind unix epoch"))
            }),
            node_id,
            locality_map,
            cas_store,
        )
    }

    /// Same as `new()`, but you can pass a custom `now_fn`, that returns a Duration since `UNIX_EPOCH`
    /// representing the current time. Used mostly in  unit tests.
    pub fn new_with_now_fn(
        config: &WorkerApiConfig,
        schedulers: &HashMap<String, Arc<dyn WorkerScheduler>>,
        now_fn: NowFn,
        node_id: [u8; 6],
        locality_map: Option<SharedBlobLocalityMap>,
        cas_store: Option<Store>,
    ) -> Result<Self, Error> {
        let scheduler = schedulers
            .get(&config.scheduler)
            .err_tip(|| {
                format!(
                    "Scheduler needs config for '{}' because it exists in worker_api",
                    config.scheduler
                )
            })?
            .clone();
        Ok(Self {
            scheduler,
            now_fn: Arc::new(now_fn),
            node_id,
            locality_map,
            cas_store,
        })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    async fn inner_connect_worker(
        &self,
        mut update_stream: impl Stream<Item = Result<UpdateForScheduler, Status>>
        + Unpin
        + Send
        + 'static,
    ) -> Result<Response<ConnectWorkerStream>, Error> {
        let first_message = update_stream
            .next()
            .await
            .err_tip(|| "Missing first message for connect_worker")?
            .err_tip(|| "Error reading first message for connect_worker")?;
        let Some(Update::ConnectWorkerRequest(connect_worker_request)) = first_message.update
        else {
            return Err(make_err!(
                Code::Internal,
                "First message was not a ConnectWorkerRequest"
            ));
        };

        let worker_cas_endpoint = connect_worker_request.cas_endpoint.clone();

        let (tx, rx) = mpsc::unbounded_channel();

        // First convert our proto platform properties into one our scheduler understands.
        let platform_properties = {
            let mut platform_properties = PlatformProperties::default();
            for property in connect_worker_request.properties {
                let platform_property_value = self
                    .scheduler
                    .get_platform_property_manager()
                    .make_prop_value(&property.name, &property.value)
                    .err_tip(|| "Bad Property during connect_worker()")?;
                platform_properties
                    .properties
                    .insert(property.name.clone(), platform_property_value);
            }
            platform_properties
        };

        // Clone tx so WorkerConnection can send messages back to the worker
        // (e.g. UploadMissingBlobs requests) independently of the scheduler.
        let worker_tx = tx.clone();

        // Now register the worker with the scheduler.
        let worker_id = {
            let worker_id = WorkerId(format!(
                "{}{}",
                connect_worker_request.worker_id_prefix,
                Uuid::now_v6(&self.node_id).hyphenated()
            ));
            let worker = Worker::new_with_cas_endpoint(
                worker_id.clone(),
                platform_properties,
                tx,
                (self.now_fn)()?.as_secs(),
                connect_worker_request.max_inflight_tasks,
                worker_cas_endpoint.clone(),
            );
            self.scheduler
                .add_worker(worker)
                .await
                .err_tip(|| "Failed to add worker in inner_connect_worker()")?;
            worker_id
        };

        WorkerConnection::start(
            self.scheduler.clone(),
            self.now_fn.clone(),
            worker_id.clone(),
            self.locality_map.clone(),
            self.cas_store.clone(),
            worker_cas_endpoint,
            worker_tx,
            update_stream,
        );

        Ok(Response::new(Box::pin(unfold(
            (rx, worker_id),
            move |state| async move {
                let (mut rx, worker_id) = state;
                if let Some(update_for_worker) = rx.recv().await {
                    return Some((Ok(update_for_worker), (rx, worker_id)));
                }
                warn!(
                    ?worker_id,
                    "UpdateForWorker channel was closed, thus closing connection to worker node",
                );

                None
            },
        ))))
    }

    pub async fn inner_connect_worker_for_testing(
        &self,
        update_stream: impl Stream<Item = Result<UpdateForScheduler, Status>> + Unpin + Send + 'static,
    ) -> Result<Response<ConnectWorkerStream>, Error> {
        self.inner_connect_worker(update_stream).await
    }
}

#[tonic::async_trait]
impl WorkerApi for WorkerApiServer {
    type ConnectWorkerStream = ConnectWorkerStream;

    #[instrument(
        err,
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn connect_worker(
        &self,
        grpc_request: tonic::Request<tonic::Streaming<UpdateForScheduler>>,
    ) -> Result<Response<Self::ConnectWorkerStream>, Status> {
        let resp = self
            .inner_connect_worker(grpc_request.into_inner())
            .await
            .map_err(Into::into);
        if resp.is_ok() {
            debug!(return = "Ok(<stream>)");
        }
        resp
    }
}

/// Maximum number of missing digests to request per UploadMissingBlobs message.
/// Keeps individual requests manageable and avoids overwhelming the worker.
const BACKFILL_BATCH_SIZE: usize = 1000;

/// Minimum seconds between backfill checks for a single worker.
/// With 10 workers sending BlobsAvailable every 100ms, this prevents
/// up to 100 has_with_results calls/sec on the server CAS.
const BACKFILL_COOLDOWN_SECS: u64 = 5;

/// Seconds after which a backfill request is considered stale and can be
/// re-requested. If a worker hasn't uploaded the blob within this window,
/// the request is assumed to have failed silently.
const BACKFILL_INFLIGHT_TIMEOUT_SECS: u64 = 60;

struct WorkerConnection {
    scheduler: Arc<dyn WorkerScheduler>,
    now_fn: Arc<NowFn>,
    worker_id: WorkerId,
    locality_map: Option<SharedBlobLocalityMap>,
    /// CAS store for checking blob existence during backfill.
    cas_store: Option<Store>,
    cas_endpoint: String,
    /// Channel to send messages back to this worker.
    worker_tx: mpsc::UnboundedSender<UpdateForWorker>,
    /// Epoch seconds of the last backfill check for this worker.
    /// Used to enforce a per-worker cooldown between backfill runs.
    last_backfill_epoch_secs: AtomicU64,
    /// Digests currently being backfilled (requested from the worker but not
    /// yet confirmed in the server CAS). Keyed by digest, value is the time
    /// the request was sent. Entries older than `BACKFILL_INFLIGHT_TIMEOUT_SECS`
    /// are considered stale and eligible for re-request.
    backfill_inflight: Arc<parking_lot::Mutex<HashMap<DigestInfo, Instant>>>,
}

impl WorkerConnection {
    fn start(
        scheduler: Arc<dyn WorkerScheduler>,
        now_fn: Arc<NowFn>,
        worker_id: WorkerId,
        locality_map: Option<SharedBlobLocalityMap>,
        cas_store: Option<Store>,
        cas_endpoint: String,
        worker_tx: mpsc::UnboundedSender<UpdateForWorker>,
        mut connection: impl Stream<Item = Result<UpdateForScheduler, Status>> + Unpin + Send + 'static,
    ) {
        let instance = Self {
            scheduler,
            now_fn,
            worker_id,
            locality_map,
            cas_store,
            cas_endpoint,
            worker_tx,
            last_backfill_epoch_secs: AtomicU64::new(0),
            backfill_inflight: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        };

        background_spawn!("worker_api", async move {
            let mut had_going_away = false;
            while let Some(maybe_update) = connection.next().await {
                let update = match maybe_update.map(|u| u.update) {
                    Ok(Some(update)) => update,
                    Ok(None) => {
                        tracing::warn!(worker_id=?instance.worker_id, "Empty update");
                        continue;
                    }
                    Err(err) => {
                        tracing::warn!(worker_id=?instance.worker_id, ?err, "Error from worker");
                        break;
                    }
                };
                let result = match update {
                    Update::ConnectWorkerRequest(_connect_worker_request) => Err(make_err!(
                        Code::Internal,
                        "Got ConnectWorkerRequest after initial message for {}",
                        instance.worker_id
                    )),
                    Update::KeepAliveRequest(keep_alive_request) => {
                        instance.inner_keep_alive(keep_alive_request).await
                    }
                    Update::GoingAwayRequest(going_away_request) => {
                        had_going_away = true;
                        instance.inner_going_away(going_away_request).await
                    }
                    Update::ExecuteResult(execute_result) => {
                        instance.inner_execution_response(execute_result).await
                    }
                    Update::ExecuteComplete(execute_complete) => {
                        instance.execution_complete(execute_complete).await
                    }
                    Update::BlobsAvailable(notification) => {
                        instance.handle_blobs_available(notification).await
                    }
                    Update::BlobsEvicted(_notification) => {
                        // Dead code path: evictions now go through
                        // BlobsAvailableNotification.evicted_digests.
                        // Kept for wire compatibility with older workers.
                        Ok(())
                    }
                };
                if let Err(err) = result {
                    tracing::warn!(worker_id=?instance.worker_id, ?err, "Error processing worker message");
                }
            }
            tracing::debug!(worker_id=?instance.worker_id, "Update for scheduler dropped");

            // Clean up locality map on disconnect.
            if !instance.cas_endpoint.is_empty() {
                if let Some(ref locality_map) = instance.locality_map {
                    locality_map.write().remove_endpoint(&instance.cas_endpoint);
                    info!(
                        worker_id=?instance.worker_id,
                        endpoint=%instance.cas_endpoint,
                        "Removed worker from blob locality map on disconnect"
                    );
                }
            }

            if !had_going_away {
                drop(instance.scheduler.remove_worker(&instance.worker_id).await);
            }
        });
    }

    async fn inner_keep_alive(&self, keep_alive_request: KeepAliveRequest) -> Result<(), Error> {
        self.scheduler
            .worker_keep_alive_received(&self.worker_id, (self.now_fn)()?.as_secs())
            .await
            .err_tip(|| "Could not process keep_alive from worker in inner_keep_alive()")?;
        let cpu_load_pct = keep_alive_request.cpu_load_pct;
        let p_core_load_pct = keep_alive_request.p_core_load_pct;
        let e_core_load_pct = keep_alive_request.e_core_load_pct;
        if cpu_load_pct > 0 || p_core_load_pct > 0 || e_core_load_pct > 0 {
            debug!(worker_id=?self.worker_id, cpu_load_pct, p_core_load_pct, e_core_load_pct, "KeepAlive received with CPU load");
            if let Err(err) = self.scheduler.update_worker_load(&self.worker_id, cpu_load_pct, p_core_load_pct, e_core_load_pct).await {
                warn!(worker_id=?self.worker_id, ?err, cpu_load_pct, p_core_load_pct, e_core_load_pct, "Failed to update worker load");
            }
        }
        Ok(())
    }

    async fn inner_going_away(&self, _going_away_request: GoingAwayRequest) -> Result<(), Error> {
        self.scheduler
            .remove_worker(&self.worker_id)
            .await
            .err_tip(|| "While calling WorkerApiServer::inner_going_away")?;
        Ok(())
    }

    fn register_action_result_digests(
        locality_map: &SharedBlobLocalityMap,
        endpoint: &str,
        execute_response: &nativelink_proto::build::bazel::remote::execution::v2::ExecuteResponse,
    ) {
        let Some(ref action_result) = execute_response.result else {
            return;
        };
        let now = SystemTime::now();
        let mut digests = Vec::new();
        for file in &action_result.output_files {
            if let Some(ref d) = file.digest {
                if let Ok(di) = DigestInfo::try_from(d.clone()) {
                    digests.push((di, now));
                }
            }
        }
        for dir in &action_result.output_directories {
            if let Some(ref d) = dir.tree_digest {
                if let Ok(di) = DigestInfo::try_from(d.clone()) {
                    digests.push((di, now));
                }
            }
        }
        if let Some(ref d) = action_result.stdout_digest {
            if d.size_bytes > 0 {
                if let Ok(di) = DigestInfo::try_from(d.clone()) {
                    digests.push((di, now));
                }
            }
        }
        if let Some(ref d) = action_result.stderr_digest {
            if d.size_bytes > 0 {
                if let Ok(di) = DigestInfo::try_from(d.clone()) {
                    digests.push((di, now));
                }
            }
        }
        if !digests.is_empty() {
            locality_map
                .write()
                .register_blobs_with_timestamps(endpoint, &digests);
        }
    }

    async fn inner_execution_response(&self, execute_result: ExecuteResult) -> Result<(), Error> {
        let operation_id = OperationId::from(execute_result.operation_id);

        match execute_result
            .result
            .err_tip(|| "Expected result to exist in ExecuteResult")?
        {
            execute_result::Result::ExecuteResponse(finished_result) => {
                // Register output digests in the locality map so the server
                // can proxy blob reads back to the worker immediately, even
                // before the BlobsAvailableNotification arrives.
                if let Some(ref locality_map) = self.locality_map {
                    if !self.cas_endpoint.is_empty() {
                        Self::register_action_result_digests(
                            locality_map,
                            &self.cas_endpoint,
                            &finished_result,
                        );
                    }
                }
                let exit_code = finished_result.result.as_ref().map_or(-1, |r| r.exit_code);
                let action_stage = finished_result
                    .try_into()
                    .err_tip(|| "Failed to convert ExecuteResponse into an ActionStage")?;
                info!(
                    worker_id=?self.worker_id,
                    %operation_id,
                    exit_code,
                    "action completed by worker"
                );
                self.scheduler
                    .update_action(
                        &self.worker_id,
                        &operation_id,
                        UpdateOperationType::UpdateWithActionStage(action_stage),
                    )
                    .await
                    .err_tip(|| format!("Failed to operation {operation_id}"))?;
            }
            execute_result::Result::InternalError(e) => {
                error!(
                    worker_id=?self.worker_id,
                    %operation_id,
                    ?e,
                    "action failed with internal error"
                );
                self.scheduler
                    .update_action(
                        &self.worker_id,
                        &operation_id,
                        UpdateOperationType::UpdateWithError(e.into()),
                    )
                    .await
                    .err_tip(|| format!("Failed to operation {operation_id}"))?;
            }
        }
        Ok(())
    }

    async fn handle_blobs_available(
        &self,
        notification: nativelink_proto::com::github::trace_machina::nativelink::remote_execution::BlobsAvailableNotification,
    ) -> Result<(), Error> {
        let cpu_load_pct = notification.cpu_load_pct;
        let p_core_load_pct = notification.p_core_load_pct;
        let e_core_load_pct = notification.e_core_load_pct;
        if cpu_load_pct > 0 || p_core_load_pct > 0 || e_core_load_pct > 0 {
            debug!(worker_id=?self.worker_id, cpu_load_pct, p_core_load_pct, e_core_load_pct, "BlobsAvailable received with CPU load");
            if let Err(err) = self.scheduler.update_worker_load(&self.worker_id, cpu_load_pct, p_core_load_pct, e_core_load_pct).await {
                warn!(worker_id=?self.worker_id, ?err, cpu_load_pct, p_core_load_pct, e_core_load_pct, "Failed to update worker load");
            }
        }

        // Update the worker's cached directory digests if any were reported (legacy path).
        if !notification.cached_directory_digests.is_empty() && !notification.is_full_subtree_snapshot {
            let cached_dirs: HashSet<DigestInfo> = notification
                .cached_directory_digests
                .iter()
                .filter_map(|d| DigestInfo::try_from(d.clone()).ok())
                .collect();
            let count = cached_dirs.len();
            debug!(worker_id=?self.worker_id, count, "BlobsAvailable received with cached directory digests");
            if let Err(err) = self.scheduler.update_cached_directories(&self.worker_id, cached_dirs).await {
                warn!(worker_id=?self.worker_id, ?err, count, "Failed to update cached directory digests");
            }
        }

        // Handle delta-encoded subtree digest updates.
        let has_subtree_update = notification.is_full_subtree_snapshot
            || !notification.added_subtree_digests.is_empty()
            || !notification.removed_subtree_digests.is_empty();
        if has_subtree_update {
            let is_full = notification.is_full_subtree_snapshot;
            let full_set: Vec<DigestInfo> = if is_full {
                notification
                    .cached_directory_digests
                    .iter()
                    .filter_map(|d| DigestInfo::try_from(d.clone()).ok())
                    .collect()
            } else {
                Vec::new()
            };
            let added: Vec<DigestInfo> = notification
                .added_subtree_digests
                .iter()
                .filter_map(|d| DigestInfo::try_from(d.clone()).ok())
                .collect();
            let removed: Vec<DigestInfo> = notification
                .removed_subtree_digests
                .iter()
                .filter_map(|d| DigestInfo::try_from(d.clone()).ok())
                .collect();
            let full_count = full_set.len();
            let added_count = added.len();
            let removed_count = removed.len();
            debug!(
                worker_id=?self.worker_id,
                is_full,
                full_count,
                added_count,
                removed_count,
                "BlobsAvailable received with subtree digest updates"
            );
            if let Err(err) = self
                .scheduler
                .update_cached_subtrees(
                    &self.worker_id,
                    is_full,
                    full_set,
                    added,
                    removed,
                )
                .await
            {
                warn!(
                    worker_id=?self.worker_id,
                    ?err,
                    is_full,
                    full_count,
                    added_count,
                    removed_count,
                    "Failed to update cached subtree digests"
                );
            }
        }

        let Some(ref locality_map) = self.locality_map else {
            return Ok(());
        };
        let endpoint = if notification.worker_cas_endpoint.is_empty() {
            &self.cas_endpoint
        } else {
            &notification.worker_cas_endpoint
        };
        if endpoint.is_empty() {
            return Ok(());
        }

        let is_full_snapshot = notification.is_full_snapshot;

        // Process evicted digests (incremental updates report evictions here).
        let evicted: Vec<DigestInfo> = notification
            .evicted_digests
            .into_iter()
            .filter_map(|d| d.try_into().ok())
            .collect();

        // Collect digests with timestamps from digest_infos (preferred).
        let mut digests_with_ts: Vec<(DigestInfo, SystemTime)> = notification
            .digest_infos
            .into_iter()
            .filter_map(|info| {
                let digest = info.digest.and_then(|d| DigestInfo::try_from(d).ok())?;
                let ts = if info.last_access_timestamp > 0 {
                    UNIX_EPOCH + Duration::from_secs(info.last_access_timestamp as u64)
                } else {
                    SystemTime::now()
                };
                Some((digest, ts))
            })
            .collect();
        // Also include plain digests for backward compatibility / simple notifications.
        let now = SystemTime::now();
        digests_with_ts.extend(
            notification
                .digests
                .into_iter()
                .filter_map(|d| DigestInfo::try_from(d).ok())
                .map(|d| (d, now)),
        );

        // Acquire the write lock once for all mutations to avoid repeated
        // lock acquisition and eliminate inconsistency windows.
        let mut map = locality_map.write();

        if is_full_snapshot {
            // Remove all existing entries for this endpoint first.
            map.remove_endpoint(endpoint);
        }

        if !evicted.is_empty() {
            info!(
                worker_id=?self.worker_id,
                endpoint,
                count=evicted.len(),
                "Processing evicted digests from BlobsAvailable"
            );
            map.evict_blobs(endpoint, &evicted);
        }

        if !digests_with_ts.is_empty() {
            info!(
                worker_id=?self.worker_id,
                endpoint,
                count=digests_with_ts.len(),
                is_full_snapshot,
                "Registering blobs available from worker"
            );
            map.register_blobs_with_timestamps(endpoint, &digests_with_ts);
        }

        // After updating the locality map, check which of the newly reported
        // blobs are missing from the server's CAS and request the worker to
        // upload them. This runs asynchronously to avoid blocking the message
        // processing loop. Only triggers on non-empty digest reports.
        //
        // Rate-limited by a per-worker cooldown to avoid excessive
        // has_with_results calls when many workers report every 100ms.
        if !digests_with_ts.is_empty() {
            if let Some(ref cas_store) = self.cas_store {
                let now_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let last = self.last_backfill_epoch_secs.load(Ordering::Relaxed);
                if now_secs.saturating_sub(last) >= BACKFILL_COOLDOWN_SECS
                    && self.last_backfill_epoch_secs.compare_exchange(
                        last, now_secs, Ordering::Relaxed, Ordering::Relaxed,
                    ).is_ok()
                {
                    let all_digests: Vec<DigestInfo> =
                        digests_with_ts.iter().map(|(d, _)| *d).collect();
                    let cas = cas_store.clone();
                    let tx = self.worker_tx.clone();
                    let worker_id = self.worker_id.clone();
                    let inflight = self.backfill_inflight.clone();
                    // Drop the locality map write lock before spawning.
                    drop(map);
                    background_spawn!("backfill_missing_blobs", async move {
                        Self::request_missing_blob_uploads(
                            &cas,
                            &tx,
                            &worker_id,
                            &all_digests,
                            &inflight,
                        )
                        .await;
                    });
                    return Ok(());
                }
            }
        }

        Ok(())
    }

    /// Check which of `digests` are missing from the server CAS and send
    /// UploadMissingBlobs requests to the worker for each batch.
    ///
    /// Deduplicates against in-flight requests: digests that were requested
    /// within the last `BACKFILL_INFLIGHT_TIMEOUT_SECS` are skipped to avoid
    /// redundant uploads. Digests that have since appeared in the CAS (or
    /// whose requests have timed out) are removed from the in-flight set.
    async fn request_missing_blob_uploads(
        cas_store: &Store,
        worker_tx: &mpsc::UnboundedSender<UpdateForWorker>,
        worker_id: &WorkerId,
        digests: &[DigestInfo],
        inflight: &parking_lot::Mutex<HashMap<DigestInfo, Instant>>,
    ) {
        if digests.is_empty() {
            return;
        }

        // Check existence on the server CAS.
        let keys: Vec<StoreKey<'_>> = digests
            .iter()
            .map(|d| StoreKey::from(*d))
            .collect();
        let mut results = vec![None; keys.len()];
        if let Err(err) = cas_store.has_with_results(&keys, &mut results).await {
            warn!(
                worker_id=?worker_id,
                ?err,
                "backfill: failed to check CAS existence"
            );
            return;
        }

        let now = Instant::now();
        let timeout = Duration::from_secs(BACKFILL_INFLIGHT_TIMEOUT_SECS);

        // Build a set of digests confirmed present in the CAS for O(1) lookup
        // during the retain loop (avoids O(inflight * digests) linear scan).
        let present_in_cas: HashSet<DigestInfo> = digests
            .iter()
            .zip(results.iter())
            .filter_map(|(d, r)| r.map(|_| *d))
            .collect();

        // Collect missing digests, filtering out those already in-flight.
        let missing: Vec<DigestInfo> = {
            let mut inflight_guard = inflight.lock();

            // Clean up: remove digests that have appeared in the CAS or
            // whose requests have timed out.
            inflight_guard.retain(|digest, requested_at| {
                // Remove if timed out.
                if now.duration_since(*requested_at) >= timeout {
                    return false;
                }
                // Remove if the digest is now present in the CAS.
                if present_in_cas.contains(digest) {
                    return false;
                }
                // Keep if still missing and not timed out.
                true
            });

            digests
                .iter()
                .zip(results.iter())
                .filter_map(|(d, r)| {
                    // Only consider digests missing from the CAS.
                    if r.is_some() {
                        return None;
                    }
                    // Skip if already in-flight.
                    if inflight_guard.contains_key(d) {
                        return None;
                    }
                    Some(*d)
                })
                .collect()
        };

        if missing.is_empty() {
            return;
        }

        info!(
            worker_id=?worker_id,
            total=digests.len(),
            missing=missing.len(),
            "backfill: requesting worker upload missing blobs"
        );

        // Record in-flight digests and send in batches.
        {
            let mut inflight_guard = inflight.lock();
            for d in &missing {
                inflight_guard.insert(*d, now);
            }
        }

        for chunk in missing.chunks(BACKFILL_BATCH_SIZE) {
            let proto_digests: Vec<Digest> = chunk
                .iter()
                .map(|d| Digest::from(*d))
                .collect();
            let msg = UpdateForWorker {
                update: Some(update_for_worker::Update::UploadMissingBlobs(
                    UploadMissingBlobsRequest {
                        digests: proto_digests,
                    },
                )),
            };
            if worker_tx.send(msg).is_err() {
                warn!(
                    worker_id=?worker_id,
                    "backfill: worker channel closed, cannot send upload request"
                );
                return;
            }
        }
    }

    async fn execution_complete(&self, execute_complete: ExecuteComplete) -> Result<(), Error> {
        let cpu_load_pct = execute_complete.cpu_load_pct;
        let p_core_load_pct = execute_complete.p_core_load_pct;
        let e_core_load_pct = execute_complete.e_core_load_pct;
        if cpu_load_pct > 0 || p_core_load_pct > 0 || e_core_load_pct > 0 {
            debug!(worker_id=?self.worker_id, cpu_load_pct, p_core_load_pct, e_core_load_pct, "ExecuteComplete received with CPU load");
            if let Err(err) = self.scheduler.update_worker_load(&self.worker_id, cpu_load_pct, p_core_load_pct, e_core_load_pct).await {
                warn!(worker_id=?self.worker_id, ?err, cpu_load_pct, p_core_load_pct, e_core_load_pct, "Failed to update worker load");
            }
        }
        let operation_id = OperationId::from(execute_complete.operation_id);
        info!(
            worker_id=?self.worker_id,
            %operation_id,
            "execution complete, CAS upload finished"
        );
        self.scheduler
            .update_action(
                &self.worker_id,
                &operation_id,
                UpdateOperationType::ExecutionComplete,
            )
            .await
            .err_tip(|| format!("Failed to operation {operation_id}"))?;
        Ok(())
    }
}
