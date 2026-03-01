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
use nativelink_error::{Code, Error, make_err};
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
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    })
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
        self.rx.changed().await.map_err(|_| {
            make_err!(
                Code::Internal,
                "Channel closed in TokioWatchActionStateResult::changed"
            )
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
