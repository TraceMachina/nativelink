// Copyright 2026 The NativeLink Authors. All rights reserved.
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

//! Repro: client keep-alive writes bypass the version CAS in the memory
//! backend, so a concurrent read-modify-write (a worker update) that
//! snapshotted the action before the keep-alive silently reverts
//! `last_client_keepalive_timestamp` — and passes the version check, so the
//! `Code::Aborted` retry defense in `SimpleSchedulerStateManager` never
//! fires. Repeated indefinitely, the client-timeout sweep kills an action
//! whose client is alive and sending keep-alives.

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use mock_instant::thread_local::MockClock;
use nativelink_config::stores::EvictionPolicy;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSubscriber, CLIENT_KEEPALIVE_DURATION,
};
use nativelink_scheduler::memory_awaited_action_db::MemoryAwaitedActionDb;
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use pretty_assertions::assert_eq;
use tokio::sync::Notify;

const INSTANCE_NAME: &str = "keepalive_clobber_instance";

fn make_action_info() -> Arc<ActionInfo> {
    let insert = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    Arc::new(ActionInfo {
        command_digest: DigestInfo::new([7; 32], 1),
        input_root_digest: DigestInfo::new([7; 32], 2),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: insert,
        insert_timestamp: insert,
        unique_qualifier: ActionUniqueQualifier::Cacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::new([7; 32], 3),
        }),
    })
}

/// Extract `last_client_keepalive_timestamp` from the Debug rendering of an
/// `AwaitedAction` (the field has no public accessor).
fn keepalive_of(awaited_action: &AwaitedAction) -> String {
    let debug = format!("{awaited_action:?}");
    let field = "last_client_keepalive_timestamp: ";
    let start = debug.find(field).expect("field in Debug output") + field.len();
    let rest = &debug[start..];
    let end = rest.find('}').expect("SystemTime Debug closes with }") + 1;
    rest[..end].to_string()
}

#[nativelink_test]
async fn client_keepalive_survives_concurrent_same_version_update() -> Result<(), Error> {
    let db = MemoryAwaitedActionDb::new(
        &EvictionPolicy::default(),
        Arc::new(Notify::new()),
        MockInstantWrapped::default,
    );

    let client_id = OperationId::from("client-keepalive");
    // The subscriber returned here carries client_info: its changed() loop is
    // what emits ActionEvent::ClientKeepAlive.
    let mut client_subscriber = db
        .add_action(
            client_id.clone(),
            make_action_info(),
            Duration::from_mins(1),
        )
        .await?;
    let operation_id = client_subscriber.borrow().await?.operation_id().clone();

    // The "worker" side: snapshot the action (version N, keepalive K0), the
    // way SimpleSchedulerStateManager does before update_awaited_action.
    let worker_subscriber = db
        .get_by_operation_id(&operation_id)
        .await?
        .expect("operation must exist");
    let stale_snapshot = worker_subscriber.borrow().await?;
    let keepalive_k0 = keepalive_of(&stale_snapshot);

    // Advance the mock clock so (a) the client's changed() loop decides it is
    // time to send a keep-alive, and (b) the stamped timestamp K1 differs
    // from K0.
    MockClock::advance(CLIENT_KEEPALIVE_DURATION + Duration::from_secs(1));

    // Drive the client subscriber: changed() immediately sends
    // ActionEvent::ClientKeepAlive, then parks awaiting a watch change.
    let changed_task = tokio::spawn(async move { client_subscriber.changed().await });

    // Wait (deterministically, bounded) for the event-handler task to stamp
    // the keep-alive into the stored AwaitedAction.
    let keepalive_k1 = {
        let mut stamped = None;
        for _ in 0..1000 {
            tokio::task::yield_now().await;
            let current = worker_subscriber.borrow().await?;
            let ts = keepalive_of(&current);
            if ts != keepalive_k0 {
                stamped = Some(ts);
                break;
            }
        }
        stamped.expect("client keep-alive was never stamped into the db")
    };
    changed_task.abort();

    // The keep-alive did NOT bump the version, so the worker's stale
    // pre-keep-alive snapshot still passes the version CAS...
    let update_result = db.update_awaited_action(stale_snapshot).await;

    // ...and send_replace overwrites the whole struct, reverting the
    // keep-alive to K0. Correct behavior would be either rejecting the stale
    // write with Code::Aborted (version bump on keep-alive) or merging the
    // keep-alive timestamp. Either way, after an accepted update the stored
    // keep-alive must still be K1.
    let after = worker_subscriber.borrow().await?;
    let keepalive_after = keepalive_of(&after);
    assert!(
        update_result.is_err() || keepalive_after == keepalive_k1,
        "Lost update: stale same-version write was accepted (result: {update_result:?}) and \
         reverted last_client_keepalive_timestamp from {keepalive_k1} back to {keepalive_after} \
         (K0 was {keepalive_k0}). The Aborted-retry defense in \
         SimpleSchedulerStateManager can never fire, so the client-timeout \
         sweep can kill an action with a live client."
    );
    // Redundant with the assert above, but gives a crisp diff on failure.
    if update_result.is_ok() {
        assert_eq!(keepalive_after, keepalive_k1);
    }
    Ok(())
}
