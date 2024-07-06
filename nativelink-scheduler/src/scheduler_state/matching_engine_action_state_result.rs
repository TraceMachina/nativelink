// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::{borrow::Cow, sync::Arc};

use async_trait::async_trait;
use nativelink_error::Error;
use nativelink_util::action_messages::{ActionInfo, ActionState};
use tokio::sync::watch;

use crate::operation_state_manager::ActionStateResult;

use super::awaited_action::AwaitedAction;

pub struct MatchingEngineActionStateResult {
    awaited_action: Arc<AwaitedAction>,
}
impl MatchingEngineActionStateResult {
    pub(crate) fn new(awaited_action: Arc<AwaitedAction>) -> Self {
        Self { awaited_action }
    }
}

#[async_trait]
impl ActionStateResult for MatchingEngineActionStateResult {
    async fn as_state(&self) -> Result<Arc<ActionState>, Error> {
        Ok(self.awaited_action.get_current_state())
    }

    async fn as_receiver(&self) -> Result<Cow<'_, watch::Receiver<Arc<ActionState>>>, Error> {
        Ok(Cow::Owned(self.awaited_action.subscribe()))
    }

    async fn as_action_info(&self) -> Result<Arc<ActionInfo>, Error> {
        Ok(self.awaited_action.get_action_info().clone())
    }
}
