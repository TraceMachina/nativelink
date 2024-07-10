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

use std::sync::Arc;

use nativelink_util::evicting_map::LenEntry;
use tokio::sync::mpsc;

use super::AwaitedAction;

/// Represents a client that is currently listening to an action.
/// When the client is dropped, it will send the [`AwaitedAction`] to the
/// `client_operation_drop_tx` if there are other cleanups needed.
#[derive(Debug)]
pub(crate) struct ClientAwaitedAction {
    /// The awaited action that the client is listening to.
    // Note: This is an Option because it is taken when the
    // ClientAwaitedAction is dropped, but will never actually be
    // None except during the drop.
    awaited_action: Option<Arc<AwaitedAction>>,

    /// The sender to notify of this struct being dropped.
    client_operation_drop_tx: mpsc::UnboundedSender<Arc<AwaitedAction>>,
}

impl ClientAwaitedAction {
    pub fn new(
        awaited_action: Arc<AwaitedAction>,
        client_operation_drop_tx: mpsc::UnboundedSender<Arc<AwaitedAction>>,
    ) -> Self {
        awaited_action.inc_listening_clients();
        Self {
            awaited_action: Some(awaited_action),
            client_operation_drop_tx,
        }
    }

    /// Returns the awaited action that the client is listening to.
    pub fn awaited_action(&self) -> &Arc<AwaitedAction> {
        self.awaited_action
            .as_ref()
            .expect("AwaitedAction should be present")
    }
}

impl Drop for ClientAwaitedAction {
    fn drop(&mut self) {
        let awaited_action = self
            .awaited_action
            .take()
            .expect("AwaitedAction should be present");
        awaited_action.dec_listening_clients();
        // If we failed to send it means noone is listening.
        let _ = self.client_operation_drop_tx.send(awaited_action);
    }
}

/// Trait to be able to use the EvictingMap with [`ClientAwaitedAction`].
/// Note: We only use EvictingMap for a time based eviction, which is
/// why the implementation has fixed default values in it.
impl LenEntry for ClientAwaitedAction {
    #[inline]
    fn len(&self) -> usize {
        0
    }

    #[inline]
    fn is_empty(&self) -> bool {
        true
    }
}
