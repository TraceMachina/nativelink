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

// The utils is used by multiple tests and some the code is dead on.
#![allow(dead_code)]

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use nativelink_error::{Code, Error};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    UpdateForWorker, update_for_worker,
};
use nativelink_util::action_messages::{
    ActionInfo, ActionState, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::operation_state_manager::ActionStateResult;
use nativelink_util::origin_event::OriginMetadata;
use tokio::sync::watch;

pub(crate) const INSTANCE_NAME: &str = "foobar_instance_name";

pub(crate) fn make_base_action_info(
    insert_timestamp: SystemTime,
    action_digest: DigestInfo,
) -> Arc<ActionInfo> {
    Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: UNIX_EPOCH,
        insert_timestamp,
        unique_qualifier: ActionUniqueQualifier::Cacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    })
}

/// Exercise the same overlapping-build contract against both scheduler databases.
pub(crate) async fn verify_overlapping_invocations(
    scheduler: &nativelink_scheduler::simple_scheduler::SimpleScheduler,
) -> Result<(), Error> {
    use nativelink_scheduler::worker::Worker;
    use nativelink_scheduler::worker_scheduler::WorkerScheduler;
    use nativelink_util::action_messages::{ActionStage, WorkerId};
    use nativelink_util::operation_state_manager::ClientStateManager;
    use nativelink_util::platform_properties::PlatformProperties;
    use tokio::sync::mpsc;

    let digest = DigestInfo::new([51; 32], 123);
    let mut listeners = Vec::new();
    let mut receivers = Vec::new();
    let mut assignments = Vec::new();
    for (invocation, scope) in [
        ("build-one", Some("1".repeat(64))),
        ("build-two", Some("2".repeat(64))),
        ("unscoped", None),
    ] {
        let worker_id = WorkerId(format!("fresh-worker-{invocation}"));
        let (tx, mut rx) = mpsc::unbounded_channel();
        // One in-flight action per worker, matching the single-use deployment.
        scheduler
            .add_worker(Worker::new(
                worker_id.clone(),
                PlatformProperties::default(),
                tx,
                10_000,
                1,
            ))
            .await?;
        assert!(matches!(
            rx.recv().await.unwrap().update,
            Some(update_for_worker::Update::ConnectionResult(_))
        ));
        let mut info = make_base_action_info(UNIX_EPOCH, digest);
        let ActionUniqueQualifier::Cacheable(key) = &mut Arc::make_mut(&mut info).unique_qualifier
        else {
            panic!("Expected cacheable action");
        };
        key.execution_scope = scope;
        let listener = scheduler
            .add_action(OperationId::from(invocation), info.clone())
            .await?;
        scheduler.do_try_match_for_test().await?;
        let update = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect(
                "Second invocation must receive a fresh worker while the first is still executing",
            )
            .expect("Worker must remain connected");
        let Some(update_for_worker::Update::StartAction(assignment)) = update.update else {
            panic!("Expected action assignment");
        };
        assert_eq!(assignment.worker_id, worker_id.to_string());
        assert_eq!(
            assignment.execute_request.as_ref().unwrap().action_digest,
            Some(digest.into())
        );
        assert_eq!(listener.as_state().await?.0.stage, ActionStage::Executing);
        // A reconnect within this invocation still follows the same execution.
        let reconnect = scheduler
            .add_action(OperationId::from(format!("{invocation}-reconnect")), info)
            .await?;
        assert_eq!(reconnect.as_state().await?.0.stage, ActionStage::Executing);
        assignments.push(assignment);
        listeners.push((listener, reconnect));
        receivers.push(rx);
    }
    assert_ne!(assignments[0].operation_id, assignments[1].operation_id);
    assert_ne!(assignments[0].worker_id, assignments[1].worker_id);
    assert_ne!(assignments[0].operation_id, assignments[2].operation_id);
    assert_ne!(assignments[1].operation_id, assignments[2].operation_id);
    for (listener, reconnect) in &listeners {
        assert_eq!(listener.as_state().await?.0.stage, ActionStage::Executing);
        assert_eq!(reconnect.as_state().await?.0.stage, ActionStage::Executing);
    }
    scheduler.do_try_match_for_test().await?;
    for rx in &mut receivers {
        assert_eq!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty));
    }
    Ok(())
}

pub(crate) struct TokioWatchActionStateResult {
    client_operation_id: OperationId,
    action_info: Arc<ActionInfo>,
    rx: watch::Receiver<Arc<ActionState>>,
}

impl TokioWatchActionStateResult {
    #[allow(dead_code, reason = "https://github.com/rust-lang/rust/issues/46379")]
    pub(crate) const fn new(
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
        rx: watch::Receiver<Arc<ActionState>>,
    ) -> Self {
        Self {
            client_operation_id,
            action_info,
            rx,
        }
    }
}

#[async_trait]
impl ActionStateResult for TokioWatchActionStateResult {
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        let mut action_state = self.rx.borrow().clone();
        Arc::make_mut(&mut action_state).client_operation_id = self.client_operation_id.clone();
        Ok((action_state, None))
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        self.rx.changed().await.map_err(|err| {
            Error::from_std_err(Code::Internal, &err)
                .append("Channel closed in TokioWatchActionStateResult::changed")
        })?;
        let mut action_state = self.rx.borrow().clone();
        Arc::make_mut(&mut action_state).client_operation_id = self.client_operation_id.clone();
        Ok((action_state, None))
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
        Ok((self.action_info.clone(), None))
    }
}

pub(crate) fn update_eq(
    expected: UpdateForWorker,
    actual: UpdateForWorker,
    ignore_id: bool,
) -> bool {
    let Some(expected_update) = expected.update else {
        return actual.update.is_none();
    };
    let Some(actual_update) = actual.update else {
        return false;
    };
    match actual_update {
        update_for_worker::Update::Disconnect(()) => {
            matches!(expected_update, update_for_worker::Update::Disconnect(()))
        }
        update_for_worker::Update::KeepAlive(()) => {
            matches!(expected_update, update_for_worker::Update::KeepAlive(()))
        }
        update_for_worker::Update::StartAction(actual_update) => match expected_update {
            update_for_worker::Update::StartAction(mut expected_update) => {
                if ignore_id {
                    expected_update
                        .operation_id
                        .clone_from(&actual_update.operation_id);
                }
                expected_update == actual_update
            }
            _ => false,
        },
        update_for_worker::Update::KillOperationRequest(actual_update) => match expected_update {
            update_for_worker::Update::KillOperationRequest(expected_update) => {
                expected_update == actual_update
            }
            _ => false,
        },
        update_for_worker::Update::ConnectionResult(actual_update) => match expected_update {
            update_for_worker::Update::ConnectionResult(expected_update) => {
                expected_update == actual_update
            }
            _ => false,
        },
    }
}
