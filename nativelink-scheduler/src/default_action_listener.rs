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

use std::pin::Pin;
use std::sync::Arc;

use futures::Future;
use nativelink_error::{make_err, Code, Error};
use nativelink_util::action_messages::{ActionState, ClientOperationId};
use tokio::sync::watch;

use crate::action_scheduler::ActionListener;

/// Simple implementation of ActionListener using tokio's watch.
pub struct DefaultActionListener {
    client_operation_id: ClientOperationId,
    action_state: watch::Receiver<Arc<ActionState>>,
}

impl DefaultActionListener {
    pub fn new(
        client_operation_id: ClientOperationId,
        mut action_state: watch::Receiver<Arc<ActionState>>,
    ) -> Self {
        action_state.mark_changed();
        Self {
            client_operation_id,
            action_state,
        }
    }

    pub async fn changed(&mut self) -> Result<Arc<ActionState>, Error> {
        self.action_state.changed().await.map_or_else(
            |e| {
                Err(make_err!(
                    Code::Internal,
                    "Sender of ActionState went away unexpectedly - {e:?}"
                ))
            },
            |()| Ok(self.action_state.borrow_and_update().clone()),
        )
    }
}

impl ActionListener for DefaultActionListener {
    fn client_operation_id(&self) -> &ClientOperationId {
        &self.client_operation_id
    }

    fn changed(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<ActionState>, Error>> + Send + '_>> {
        Box::pin(self.changed())
    }
}

impl Clone for DefaultActionListener {
    fn clone(&self) -> Self {
        let mut action_state = self.action_state.clone();
        action_state.mark_changed();
        Self {
            client_operation_id: self.client_operation_id.clone(),
            action_state,
        }
    }
}
