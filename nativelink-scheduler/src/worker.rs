// Copyright 2023 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    update_for_worker, ConnectionResult, StartExecute, UpdateForWorker,
};
use nativelink_util::action_messages::{ActionInfo, OperationId, WorkerId};
use nativelink_util::metrics_utils::{
    CollectorState, CounterWithTime, FuncCounterWrapper, MetricsComponent,
};
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use tokio::sync::mpsc::UnboundedSender;

pub type WorkerTimestamp = u64;

/// Notifications to send worker about a requested state change.
pub enum WorkerUpdate {
    /// Requests that the worker begin executing this action.
    RunAction((OperationId, Arc<ActionInfo>)),

    /// Request that the worker is no longer in the pool and may discard any jobs.
    Disconnect,
}

/// Represents a connection to a worker and used as the medium to
/// interact with the worker from the client/scheduler.
pub struct Worker {
    /// Unique identifier of the worker.
    pub id: WorkerId,

    /// Properties that describe the capabilities of this worker.
    pub platform_properties: PlatformProperties,

    /// Channel to send commands from scheduler to worker.
    pub tx: UnboundedSender<UpdateForWorker>,

    /// The action info of the running actions on the worker
    pub running_action_infos: HashMap<OperationId, Arc<ActionInfo>>,

    /// Timestamp of last time this worker had been communicated with.
    // Warning: Do not update this timestamp without updating the placement of the worker in
    // the LRUCache in the Workers struct.
    pub last_update_timestamp: WorkerTimestamp,

    /// Whether the worker rejected the last action due to back pressure.
    pub is_paused: bool,

    /// Whether the worker is draining.
    pub is_draining: bool,

    /// Stats about the worker.
    metrics: Arc<Metrics>,
}

fn send_msg_to_worker(
    tx: &mut UnboundedSender<UpdateForWorker>,
    msg: update_for_worker::Update,
) -> Result<(), Error> {
    tx.send(UpdateForWorker { update: Some(msg) })
        .map_err(|_| make_err!(Code::Internal, "Worker disconnected"))
}

/// Reduces the platform properties available on the worker based on the platform properties provided.
/// This is used because we allow more than 1 job to run on a worker at a time, and this is how the
/// scheduler knows if more jobs can run on a given worker.
fn reduce_platform_properties(
    parent_props: &mut PlatformProperties,
    reduction_props: &PlatformProperties,
) {
    debug_assert!(reduction_props.is_satisfied_by(parent_props));
    for (property, prop_value) in &reduction_props.properties {
        if let PlatformPropertyValue::Minimum(value) = prop_value {
            let worker_props = &mut parent_props.properties;
            if let &mut PlatformPropertyValue::Minimum(worker_value) =
                &mut worker_props.get_mut(property).unwrap()
            {
                *worker_value -= value;
            }
        }
    }
}

impl Worker {
    pub fn new(
        id: WorkerId,
        platform_properties: PlatformProperties,
        tx: UnboundedSender<UpdateForWorker>,
        timestamp: WorkerTimestamp,
    ) -> Self {
        Self {
            id,
            platform_properties,
            tx,
            running_action_infos: HashMap::new(),
            last_update_timestamp: timestamp,
            is_paused: false,
            is_draining: false,
            metrics: Arc::new(Metrics {
                connected_timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                actions_completed: CounterWithTime::default(),
                run_action: FuncCounterWrapper::default(),
                keep_alive: FuncCounterWrapper::default(),
                notify_disconnect: CounterWithTime::default(),
            }),
        }
    }

    /// Sends the initial connection information to the worker. This generally is just meta info.
    /// This should only be sent once and should always be the first item in the stream.
    pub fn send_initial_connection_result(&mut self) -> Result<(), Error> {
        send_msg_to_worker(
            &mut self.tx,
            update_for_worker::Update::ConnectionResult(ConnectionResult {
                worker_id: self.id.to_string(),
            }),
        )
        .err_tip(|| format!("Failed to send ConnectionResult to worker : {}", self.id))
    }

    /// Notifies the worker of a requested state change.
    pub fn notify_update(&mut self, worker_update: WorkerUpdate) -> Result<(), Error> {
        match worker_update {
            WorkerUpdate::RunAction((operation_id, action_info)) => {
                self.run_action(operation_id, action_info)
            }
            WorkerUpdate::Disconnect => {
                self.metrics.notify_disconnect.inc();
                send_msg_to_worker(&mut self.tx, update_for_worker::Update::Disconnect(()))
            }
        }
    }

    pub fn keep_alive(&mut self) -> Result<(), Error> {
        let tx = &mut self.tx;
        let id = self.id;
        self.metrics.keep_alive.wrap(move || {
            send_msg_to_worker(tx, update_for_worker::Update::KeepAlive(()))
                .err_tip(|| format!("Failed to send KeepAlive to worker : {id}"))
        })
    }

    fn run_action(
        &mut self,
        operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<(), Error> {
        let tx = &mut self.tx;
        let worker_platform_properties = &mut self.platform_properties;
        let running_action_infos = &mut self.running_action_infos;
        self.metrics.run_action.wrap(move || {
            let action_info_clone = action_info.as_ref().clone();
            let operation_id_string = operation_id.to_string();
            running_action_infos.insert(operation_id, action_info.clone());
            reduce_platform_properties(
                worker_platform_properties,
                &action_info.platform_properties,
            );
            send_msg_to_worker(
                tx,
                update_for_worker::Update::StartAction(StartExecute {
                    execute_request: Some(action_info_clone.into()),
                    operation_id: operation_id_string,
                    queued_timestamp: Some(action_info.insert_timestamp.into()),
                }),
            )
        })
    }

    pub(crate) fn complete_action(&mut self, operation_id: &OperationId) -> Result<(), Error> {
        let action_info = self.running_action_infos.remove(operation_id).err_tip(|| {
            format!(
                "Worker {} tried to complete operation {} that was not running",
                self.id, operation_id
            )
        })?;
        self.restore_platform_properties(&action_info.platform_properties);
        self.is_paused = false;
        self.metrics.actions_completed.inc();
        Ok(())
    }

    pub fn has_actions(&self) -> bool {
        !self.running_action_infos.is_empty()
    }

    fn restore_platform_properties(&mut self, props: &PlatformProperties) {
        for (property, prop_value) in &props.properties {
            if let PlatformPropertyValue::Minimum(value) = prop_value {
                let worker_props = &mut self.platform_properties.properties;
                if let PlatformPropertyValue::Minimum(worker_value) =
                    worker_props.get_mut(property).unwrap()
                {
                    *worker_value += value;
                }
            }
        }
    }

    pub fn can_accept_work(&self) -> bool {
        !self.is_paused && !self.is_draining
    }
}

impl PartialEq for Worker {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Worker {}

impl Hash for Worker {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

#[derive(Default)]
struct Metrics {
    connected_timestamp: u64,
    actions_completed: CounterWithTime,
    run_action: FuncCounterWrapper,
    keep_alive: FuncCounterWrapper,
    notify_disconnect: CounterWithTime,
}

impl MetricsComponent for Worker {
    fn gather_metrics(&self, c: &mut CollectorState) {
        c.publish_with_labels(
            "connected_timestamp",
            &self.metrics.connected_timestamp,
            "The timestamp of when this worker connected.",
            vec![("worker_id".into(), format!("{}", self.id).into())],
        );
        c.publish_with_labels(
            "actions_completed",
            &self.metrics.actions_completed,
            "The number of actions completed for this worker.",
            vec![("worker_id".into(), format!("{}", self.id).into())],
        );
        c.publish_with_labels(
            "run_action",
            &self.metrics.run_action,
            "The number of actions started for this worker.",
            vec![("worker_id".into(), format!("{}", self.id).into())],
        );
        c.publish_with_labels(
            "keep_alive",
            &self.metrics.keep_alive,
            "The number of keep_alive sent to this worker.",
            vec![("worker_id".into(), format!("{}", self.id).into())],
        );
        c.publish_with_labels(
            "notify_disconnect",
            &self.metrics.notify_disconnect,
            "The number of notify_disconnect sent to this worker.",
            vec![("worker_id".into(), format!("{}", self.id).into())],
        );

        // Publish info about current state of worker.
        c.publish_with_labels(
            "is_paused",
            &self.is_paused,
            "If this worker is paused.",
            vec![("worker_id".into(), format!("{}", self.id).into())],
        );
        c.publish_with_labels(
            "is_draining",
            &self.is_draining,
            "If this worker is draining.",
            vec![("worker_id".into(), format!("{}", self.id).into())],
        );
        for action_info in self.running_action_infos.values() {
            let action_name = action_info.unique_qualifier.to_string();
            c.publish_with_labels(
                "timeout",
                &action_info.timeout,
                "Timeout of the running action.",
                vec![("digest".into(), action_name.clone().into())],
            );
            c.publish_with_labels(
                "priority",
                &action_info.priority,
                "Priority of the running action.",
                vec![("digest".into(), action_name.clone().into())],
            );
            c.publish_with_labels(
                "load_timestamp",
                &action_info.load_timestamp,
                "When this action started to be loaded from the CAS.",
                vec![("digest".into(), action_name.clone().into())],
            );
            c.publish_with_labels(
                "insert_timestamp",
                &action_info.insert_timestamp,
                "When this action was created.",
                vec![("digest".into(), action_name.clone().into())],
            );
        }
        for (prop_name, prop_type_and_value) in &self.platform_properties.properties {
            match prop_type_and_value {
                PlatformPropertyValue::Exact(value)
                | PlatformPropertyValue::Priority(value)
                | PlatformPropertyValue::Unknown(value) => {
                    c.publish_with_labels(
                        "platform_properties",
                        value,
                        "The platform properties state.",
                        vec![("property_name".into(), prop_name.to_string().into())],
                    );
                }
                PlatformPropertyValue::Minimum(value) => {
                    c.publish_with_labels(
                        "platform_properties",
                        value,
                        "The platform properties state.",
                        vec![("property_name".into(), prop_name.to_string().into())],
                    );
                }
            };
        }
    }
}
