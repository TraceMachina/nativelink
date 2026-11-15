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

//! The network between the simulated clients and workers and the real
//! servers. A worker connects through `InProcessWorkerClient`, which hands
//! its messages to `WorkerApiServer::inner_connect_worker_for_testing` and
//! turns the server's stream back into the `tonic::Streaming` the worker
//! reads, through a `ChannelBody`. Every message crosses the trace on the
//! way, when it is delivered. A link can be cut (the connection closes) or
//! silenced (it stays open and holds what is sent until it is back, as TCP
//! does across a blip), and a scheduler restart cuts them all.

use core::sync::atomic::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use hyper::body::Frame;
use nativelink_error::{Error, ResultExt, make_err};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_scheduler::Update as SchedulerUpdate;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_worker::Update as WorkerUpdate;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    ConnectWorkerRequest, ExecuteAccepted, ExecuteComplete, ExecuteDeclined, ExecuteResult,
    GoingAwayRequest, KeepAliveRequest, StartExecute, UpdateForScheduler, UpdateForWorker,
    execute_result,
};
use nativelink_service::execution_server::ExecutionServer;
use nativelink_service::worker_api_server::WorkerApiServer;
use nativelink_util::channel_body_for_tests::ChannelBody;
use nativelink_util::common::encode_stream_proto;
use nativelink_util::background_spawn;
use nativelink_worker::worker_api_client_wrapper::WorkerApiClientTrait;
use parking_lot::Mutex;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::codec::Codec;
use tonic::{Response, Status, Streaming};
use tonic_prost::ProstCodec;

use crate::actions::{ActionSource, Plan, Registry, start_execute_digest};
use crate::trace::{Event, Trace};

/// What a worker's link carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Link {
    #[default]
    Up,
    /// Open, but nothing gets through in either direction until the link
    /// is up again; then everything held is delivered in order.
    Silent,
    /// The worker's `ExecuteAccepted` messages are lost.
    DropAccepts,
    /// The worker's results reach the scheduler twice.
    DuplicateResults,
}

#[derive(Debug)]
struct LinkState {
    link: watch::Sender<Link>,
    /// Bumped to close the worker's current connection.
    cuts: watch::Sender<u64>,
}

impl Default for LinkState {
    fn default() -> Self {
        Self {
            link: watch::channel(Link::Up).0,
            cuts: watch::channel(0).0,
        }
    }
}

/// The servers a client or worker reaches, and the state every connection
/// shares.
pub struct Net {
    worker_api: Mutex<Option<Arc<WorkerApiServer>>>,
    execution: Mutex<Option<ExecutionServer>>,
    /// Bumped when the scheduler goes down; every connection closes.
    epoch: watch::Sender<u64>,
    links: Mutex<HashMap<String, LinkState>>,
    /// Scheduler operation id to the scenario's action label.
    actions: Mutex<HashMap<String, String>>,
    /// Scheduler operation id to a stable key, `<action>#<n>` for the
    /// action's `n`th operation, since the ids themselves can be random.
    keys: Mutex<HashMap<String, String>>,
    /// Operations seen per action.
    per_action: Mutex<HashMap<String, u64>>,
    registry: Arc<Registry>,
    trace: Trace,
}

impl core::fmt::Debug for Net {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Net").finish_non_exhaustive()
    }
}

impl Net {
    pub(crate) fn new(registry: Arc<Registry>, trace: Trace) -> Arc<Self> {
        Arc::new(Self {
            worker_api: Mutex::new(None),
            execution: Mutex::new(None),
            epoch: watch::channel(0).0,
            links: Mutex::new(HashMap::new()),
            actions: Mutex::new(HashMap::new()),
            keys: Mutex::new(HashMap::new()),
            per_action: Mutex::new(HashMap::new()),
            registry,
            trace,
        })
    }

    pub const fn trace(&self) -> &Trace {
        &self.trace
    }

    pub(crate) fn serve(&self, worker_api: Arc<WorkerApiServer>, execution: ExecutionServer) {
        *self.worker_api.lock() = Some(worker_api);
        *self.execution.lock() = Some(execution);
    }

    /// Takes the servers away and closes every connection to them.
    pub(crate) fn unserve(&self) {
        self.worker_api.lock().take();
        self.execution.lock().take();
        self.epoch.send_modify(|epoch| *epoch += 1);
    }

    pub(crate) fn execution(&self) -> Option<ExecutionServer> {
        self.execution.lock().clone()
    }

    pub(crate) fn epoch(&self) -> watch::Receiver<u64> {
        self.epoch.subscribe()
    }

    pub(crate) fn set_link(&self, worker: &str, link: Link) {
        self.links
            .lock()
            .entry(worker.to_string())
            .or_default()
            .link
            .send_replace(link);
    }

    fn link(&self, worker: &str) -> Link {
        self.links
            .lock()
            .get(worker)
            .map(|l| *l.link.borrow())
            .unwrap_or_default()
    }

    fn links(&self, worker: &str) -> watch::Receiver<Link> {
        self.links
            .lock()
            .entry(worker.to_string())
            .or_default()
            .link
            .subscribe()
    }

    pub(crate) fn cut(&self, worker: &str) {
        self.links
            .lock()
            .entry(worker.to_string())
            .or_default()
            .cuts
            .send_modify(|cuts| *cuts += 1);
    }

    fn cuts(&self, worker: &str) -> watch::Receiver<u64> {
        self.links
            .lock()
            .entry(worker.to_string())
            .or_default()
            .cuts
            .subscribe()
    }

    /// The stable key of a scheduler operation id for `action`, minted on
    /// first sight.
    pub(crate) fn key_of(&self, operation_id: &str, action: &str) -> String {
        self.keys
            .lock()
            .entry(operation_id.to_string())
            .or_insert_with(|| {
                let mut per_action = self.per_action.lock();
                let n = per_action.entry(action.to_string()).or_default();
                *n += 1;
                format!("{action}#{n}")
            })
            .clone()
    }

    fn action_of(&self, operation_id: &str) -> String {
        self.actions
            .lock()
            .get(operation_id)
            .cloned()
            .unwrap_or_else(|| format!("?{operation_id}"))
    }

    /// Records a message from the scheduler to `worker`.
    fn on_update_for_worker(&self, worker: &str, update: &UpdateForWorker) {
        match &update.update {
            Some(WorkerUpdate::ConnectionResult(result)) => {
                self.trace.record(Event::WorkerConnected {
                    worker: worker.to_string(),
                    dispatch_ack: result.dispatch_ack,
                });
            }
            Some(WorkerUpdate::StartAction(start_execute)) => {
                let action = start_execute_digest(start_execute)
                    .and_then(|digest| self.registry.label(&digest))
                    .unwrap_or_else(|| "?".to_string());
                self.actions
                    .lock()
                    .insert(start_execute.operation_id.clone(), action.clone());
                let key = self.key_of(&start_execute.operation_id, &action);
                self.trace.record(Event::Dispatch {
                    worker: worker.to_string(),
                    action,
                    key,
                });
            }
            Some(WorkerUpdate::KillOperationRequest(kill)) => {
                self.trace.record(Event::Kill {
                    worker: worker.to_string(),
                    action: self.action_of(&kill.operation_id),
                });
            }
            Some(WorkerUpdate::KeepAlive(()) | WorkerUpdate::Disconnect(())) | None => {}
        }
    }

    fn on_update_for_scheduler(&self, worker: &str, update: &SchedulerUpdate) {
        let worker = worker.to_string();
        let event = match update {
            SchedulerUpdate::ExecuteAccepted(accepted) => Event::Accept {
                worker,
                action: self.action_of(&accepted.operation_id),
            },
            SchedulerUpdate::ExecuteDeclined(declined) => Event::Decline {
                worker,
                action: self.action_of(&declined.operation_id),
            },
            SchedulerUpdate::ExecuteComplete(complete) => Event::Complete {
                worker,
                action: self.action_of(&complete.operation_id),
            },
            SchedulerUpdate::ExecuteResult(result) => Event::Result {
                worker,
                action: self.action_of(&result.operation_id),
                ok: matches!(
                    result.result,
                    Some(execute_result::Result::ExecuteResponse(_))
                ),
            },
            SchedulerUpdate::ConnectWorkerRequest(_)
            | SchedulerUpdate::KeepAliveRequest(_)
            | SchedulerUpdate::GoingAwayRequest(_) => return,
        };
        self.trace.record(event);
    }
}

impl ActionSource for Net {
    fn plan(&self, start_execute: &StartExecute) -> Result<Plan, Error> {
        let digest = start_execute_digest(start_execute)
            .err_tip(|| "StartExecute without an action digest")?;
        let entry = self
            .registry
            .lookup(&digest)
            .err_tip(|| format!("No simulated action for {digest}"))?;
        let attempt = entry.attempts.fetch_add(1, Ordering::AcqRel) as usize;
        let label = entry.action.label();
        Ok(Plan {
            key: self.key_of(&start_execute.operation_id, &label),
            action: label,
            prepare: entry.action.prepare,
            run_for: entry.action.run,
            upload: entry.action.upload,
            run: entry.action.attempts[attempt.min(entry.action.attempts.len() - 1)],
        })
    }

    fn record(&self, event: Event) {
        self.trace.record(event);
    }
}

/// The worker's end of one connection: where its messages go, and what a
/// silent link holds back.
#[derive(Debug, Default)]
struct OutboxState {
    tx: Option<mpsc::UnboundedSender<Result<UpdateForScheduler, Status>>>,
    held: Vec<SchedulerUpdate>,
}

type Outbox = Arc<Mutex<OutboxState>>;

/// Delivers `update` from `worker` to the scheduler, as many times as the
/// link says.
fn deliver_up(
    net: &Net,
    worker: &str,
    tx: &mpsc::UnboundedSender<Result<UpdateForScheduler, Status>>,
    update: SchedulerUpdate,
) -> Result<(), ()> {
    let copies = match (net.link(worker), &update) {
        (Link::DropAccepts, SchedulerUpdate::ExecuteAccepted(_)) => 0,
        (Link::DuplicateResults, SchedulerUpdate::ExecuteResult(_)) => 2,
        _ => 1,
    };
    // A copy the network made is not the worker's doing: the trace
    // records what the worker sent.
    if copies > 0 {
        net.on_update_for_scheduler(worker, &update);
    }
    let mut update = Some(update);
    for copy in 1..=copies {
        let message = if copy == copies {
            update.take()
        } else {
            update.clone()
        };
        tx.send(Ok(UpdateForScheduler { update: message }))
            .map_err(drop)?;
    }
    Ok(())
}

/// `WorkerApiClientTrait` over the in-process `Net`.
#[derive(Clone, Debug)]
pub struct InProcessWorkerClient {
    worker: String,
    net: Arc<Net>,
    outbox: Outbox,
}

impl InProcessWorkerClient {
    pub(crate) fn connect(worker: &str, net: &Arc<Net>) -> Result<Self, Error> {
        if net.worker_api.lock().is_none() {
            return Err(make_err!(
                nativelink_error::Code::Unavailable,
                "No scheduler is serving"
            ));
        }
        Ok(Self {
            worker: worker.to_string(),
            net: net.clone(),
            outbox: Arc::new(Mutex::new(OutboxState::default())),
        })
    }

    async fn send(&self, update: SchedulerUpdate) -> Result<(), Error> {
        let disconnected = || {
            make_err!(
                nativelink_error::Code::Unavailable,
                "worker update with disconnected channel"
            )
        };
        let tx = {
            let mut outbox = self.outbox.lock();
            let tx = outbox.tx.clone().ok_or_else(disconnected)?;
            if self.net.link(&self.worker) == Link::Silent {
                outbox.held.push(update);
                return Ok(());
            }
            tx
        };
        deliver_up(&self.net, &self.worker, &tx, update).map_err(|()| {
            self.outbox.lock().tx.take();
            disconnected()
        })
    }
}

impl WorkerApiClientTrait for InProcessWorkerClient {
    async fn connect_worker(
        &mut self,
        request: ConnectWorkerRequest,
    ) -> Result<Response<Streaming<UpdateForWorker>>, Status> {
        let server = self
            .net
            .worker_api
            .lock()
            .clone()
            .ok_or_else(|| Status::unavailable("No scheduler is serving"))?;
        let mut epoch = self.net.epoch();
        let start_epoch = *epoch.borrow_and_update();
        let mut cuts = self.net.cuts(&self.worker);
        let start_cuts = *cuts.borrow_and_update();

        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(Ok(UpdateForScheduler {
            update: Some(SchedulerUpdate::ConnectWorkerRequest(request)),
        }))
        .map_err(|_| Status::internal("Could not queue ConnectWorkerRequest"))?;
        *self.outbox.lock() = OutboxState {
            tx: Some(tx.clone()),
            held: Vec::new(),
        };

        let mut from_server = server
            .inner_connect_worker_for_testing(UnboundedReceiverStream::new(rx))
            .await
            .map_err(Status::from)?
            .into_inner();

        let (frames, body) = ChannelBody::new();
        let net = self.net.clone();
        let worker = self.worker.clone();
        let outbox = self.outbox.clone();
        let mut links = self.net.links(&self.worker);
        // The connection lives in this task: it ends when the server's stream
        // ends, the link is cut or the scheduler goes down, and closing it
        // closes both directions.
        drop(background_spawn!("sim_worker_link", async move {
            let deliver_down = async |update: UpdateForWorker| -> bool {
                net.on_update_for_worker(&worker, &update);
                let Ok(bytes) = encode_stream_proto(&update) else {
                    return false;
                };
                frames.send(Frame::data(bytes)).await.is_ok()
            };
            let mut held_down: Vec<UpdateForWorker> = Vec::new();
            loop {
                tokio::select! {
                    update = from_server.next() => {
                        let Some(Ok(update)) = update else { break };
                        if net.link(&worker) == Link::Silent {
                            held_down.push(update);
                            continue;
                        }
                        if !deliver_down(update).await {
                            break;
                        }
                    }
                    changed = links.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        if *links.borrow_and_update() == Link::Silent {
                            continue;
                        }
                        // Back up: what was held goes through, in order.
                        let mut delivered = true;
                        for update in held_down.drain(..) {
                            if !deliver_down(update).await {
                                delivered = false;
                                break;
                            }
                        }
                        let held_up = core::mem::take(&mut outbox.lock().held);
                        for update in held_up {
                            if deliver_up(&net, &worker, &tx, update).is_err() {
                                delivered = false;
                                break;
                            }
                        }
                        if !delivered {
                            break;
                        }
                    }
                    changed = epoch.changed() => {
                        if changed.is_err() || *epoch.borrow() != start_epoch {
                            break;
                        }
                    }
                    changed = cuts.changed() => {
                        if changed.is_err() || *cuts.borrow() != start_cuts {
                            break;
                        }
                    }
                }
            }
            *outbox.lock() = OutboxState::default();
            drop(tx);
            net.trace().record(Event::WorkerDisconnected { worker });
        }));

        let mut codec = ProstCodec::<UpdateForWorker, UpdateForWorker>::default();
        Ok(Response::new(Streaming::new_request(
            codec.decoder(),
            body,
            None,
            None,
        )))
    }

    async fn keep_alive(&mut self, request: KeepAliveRequest) -> Result<(), Error> {
        self.send(SchedulerUpdate::KeepAliveRequest(request)).await
    }

    async fn going_away(&mut self, request: GoingAwayRequest) -> Result<(), Error> {
        self.send(SchedulerUpdate::GoingAwayRequest(request)).await
    }

    async fn execution_response(&mut self, request: ExecuteResult) -> Result<(), Error> {
        self.send(SchedulerUpdate::ExecuteResult(request)).await
    }

    async fn execute_accepted(&mut self, request: ExecuteAccepted) -> Result<(), Error> {
        self.send(SchedulerUpdate::ExecuteAccepted(request)).await
    }

    async fn execute_declined(&mut self, request: ExecuteDeclined) -> Result<(), Error> {
        self.send(SchedulerUpdate::ExecuteDeclined(request)).await
    }

    async fn execution_complete(&mut self, request: ExecuteComplete) -> Result<(), Error> {
        self.send(SchedulerUpdate::ExecuteComplete(request)).await
    }
}
