// Copyright 2026 The NativeLink Authors. All rights reserved.
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

use core::time::Duration;

use nativelink_macro::nativelink_test;
use nativelink_util::shutdown_guard::{Priority, ShutdownGuard};
use tokio::sync::broadcast;

/// The original's wait must hold while a clone is alive and return once
/// it is dropped. Before, the promotion inside `wait_for` took one off the
/// clone's count, so a single clone looked already gone.
#[nativelink_test]
async fn wait_for_holds_until_the_clone_drops() {
    let mut original = ShutdownGuard::default();
    let clone = original.clone();

    let held =
        tokio::time::timeout(Duration::from_millis(50), original.wait_for(Priority::P0)).await;
    assert!(
        held.is_err(),
        "wait_for returned while a clone was still alive"
    );

    drop(clone);
    tokio::time::timeout(Duration::from_secs(1), original.wait_for(Priority::P0))
        .await
        .expect("wait_for should return once the clone is dropped");
}

/// The production path: the SIGTERM handler broadcasts a clone, a listener
/// receives it (another clone), the buffered value is dropped, and the
/// handler waits. The listener's guard is what has to hold the process.
#[nativelink_test]
async fn a_guard_received_over_a_broadcast_holds_the_wait() {
    let mut original = ShutdownGuard::default();
    let (tx, mut rx) = broadcast::channel::<ShutdownGuard>(4);
    tx.send(original.clone()).expect("send");
    let listener_guard = rx.recv().await.expect("recv");
    drop(tx);
    drop(rx);

    let held =
        tokio::time::timeout(Duration::from_millis(50), original.wait_for(Priority::P0)).await;
    assert!(
        held.is_err(),
        "wait_for returned while the listener still held its guard"
    );

    drop(listener_guard);
    tokio::time::timeout(Duration::from_secs(1), original.wait_for(Priority::P0))
        .await
        .expect("wait_for should return once the listener drops its guard");
}
