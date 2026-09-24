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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::{PropertyType, SimpleSpec};
use nativelink_config::stores::RedisSpec;
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_redis_tester::FakeRedisBackend;
use nativelink_scheduler::awaited_action_db::AwaitedActionDb;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_store::redis_store::{RedisStore, RedisSubscriptionManager};
use nativelink_util::action_messages::{ActionStage, OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::ClientStateManager;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use nativelink_util::store_trait::SchedulerStore;
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW_TIME: u64 = 10_000;

const TTL: Duration = Duration::from_secs(15);

fn shape(gpu_count: u64) -> PlatformProperties {
    PlatformProperties::new(HashMap::from([(
        "gpu_count".to_string(),
        PlatformPropertyValue::Minimum(gpu_count),
    )]))
}

async fn make_db(
    fake_redis_backend: &FakeRedisBackend<RedisSubscriptionManager>,
    port: u16,
) -> Result<impl AwaitedActionDb, Error> {
    let spec = RedisSpec {
        addresses: vec![format!("redis://127.0.0.1:{port}")],
        experimental_pub_sub_channel: Some("sub_channel".to_string()),
        ..Default::default()
    };
    let store = RedisStore::new_standard(spec).await?;
    fake_redis_backend.set_subscription_manager(store.subscription_manager().await?);
    StoreAwaitedActionDb::new(
        store,
        Arc::new(Notify::new()),
        MockInstantWrapped::default,
        OperationId::default,
        60,
        60,
        false,
    )
    .await
}

#[nativelink_test]
async fn schedulers_see_each_others_fleet_but_not_their_own() -> Result<(), Error> {
    let fake_redis_backend: FakeRedisBackend<RedisSubscriptionManager> = FakeRedisBackend::new();
    let port = fake_redis_backend.clone().run().await;
    let db_a = make_db(&fake_redis_backend, port).await?;
    let db_b = make_db(&fake_redis_backend, port).await?;

    // Nobody else has published yet.
    let peers = db_a
        .exchange_fleet_capabilities("a", vec![shape(0)], TTL)
        .await?;
    assert_eq!(peers, Vec::<PlatformProperties>::new());

    let peers = db_b
        .exchange_fleet_capabilities("b", vec![shape(1), shape(2)], TTL)
        .await?;
    assert_eq!(peers, vec![shape(0)]);

    let mut peers = db_a
        .exchange_fleet_capabilities("a", vec![shape(0)], TTL)
        .await?;
    peers.sort_by_key(|p| p.properties["gpu_count"].as_str().into_owned());
    assert_eq!(peers, vec![shape(1), shape(2)]);

    // Each record expires on its own once its scheduler stops refreshing it.
    let expiries = fake_redis_backend.expiries.lock().unwrap();
    assert_eq!(expiries.get("fc_a"), Some(&15));
    assert_eq!(expiries.get("fc_b"), Some(&15));
    Ok(())
}

#[nativelink_test]
async fn republishing_replaces_the_previous_record() -> Result<(), Error> {
    let fake_redis_backend: FakeRedisBackend<RedisSubscriptionManager> = FakeRedisBackend::new();
    let port = fake_redis_backend.clone().run().await;
    let db_a = make_db(&fake_redis_backend, port).await?;
    let db_b = make_db(&fake_redis_backend, port).await?;

    db_b.exchange_fleet_capabilities("b", vec![shape(1)], TTL)
        .await?;
    db_b.exchange_fleet_capabilities("b", vec![], TTL).await?;

    let peers = db_a.exchange_fleet_capabilities("a", vec![], TTL).await?;
    assert_eq!(peers, Vec::<PlatformProperties>::new());
    Ok(())
}

/// Two schedulers on one Redis. Only B has a GPU worker, and it is busy.
/// A must keep the GPU action queued while B's worker exists, and fail it
/// once that worker is gone.
#[nativelink_test]
async fn scheduler_defers_to_a_peers_worker_it_cannot_see() -> Result<(), Error> {
    const TIMEOUT_S: u64 = 2;
    const EXCHANGE_INTERVAL: Duration = Duration::from_millis(50);
    // Enough exchanges for both schedulers to publish and read each other.
    const SETTLE: Duration = Duration::from_millis(300);

    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let fake_redis_backend: FakeRedisBackend<RedisSubscriptionManager> = FakeRedisBackend::new();
    let port = fake_redis_backend.clone().run().await;

    let spec = SimpleSpec {
        supported_platform_properties: Some(HashMap::from([(
            "gpu_count".to_string(),
            PropertyType::Minimum,
        )])),
        unsatisfiable_action_timeout_s: TIMEOUT_S,
        // The mock clock jumps past the defaults for these.
        client_action_timeout_s: 1_000_000,
        worker_timeout_s: 1_000_000,
        ..Default::default()
    };
    let make_scheduler = |db| {
        SimpleScheduler::new_with_fleet_exchange_interval(
            &spec,
            db,
            || async move {},
            Arc::new(Notify::new()),
            MockInstantWrapped::default,
            None,
            EXCHANGE_INTERVAL,
        )
        .0
    };
    let scheduler_a = make_scheduler(make_db(&fake_redis_backend, port).await?);
    let scheduler_b = make_scheduler(make_db(&fake_redis_backend, port).await?);

    let cpu_worker = shape(0);
    let gpu_worker = shape(1);
    let (cpu_tx, _cpu_rx) = mpsc::unbounded_channel();
    scheduler_a
        .add_worker(Worker::new(
            WorkerId("cpu".to_string()),
            cpu_worker,
            cpu_tx,
            NOW_TIME,
            1,
        ))
        .await?;
    let (gpu_tx, _gpu_rx) = mpsc::unbounded_channel();
    scheduler_b
        .add_worker(Worker::new(
            WorkerId("gpu".to_string()),
            gpu_worker,
            gpu_tx,
            NOW_TIME,
            1,
        ))
        .await?;

    let gpu_action = |digest_byte: u8| {
        let mut action_info = make_base_action_info(
            UNIX_EPOCH + Duration::from_secs(NOW_TIME),
            DigestInfo::new([digest_byte; 32], 512),
        );
        Arc::make_mut(&mut action_info).platform_properties =
            HashMap::from([("gpu_count".to_string(), "1".to_string())]);
        action_info
    };

    // B's GPU worker takes the first action and has no room for another.
    let running = scheduler_b
        .add_action(OperationId::default(), gpu_action(1))
        .await?;
    scheduler_b.do_try_match_for_test().await?;
    assert_eq!(running.as_state().await?.0.stage, ActionStage::Executing);

    let waiting = scheduler_a
        .add_action(OperationId::default(), gpu_action(2))
        .await?;
    tokio::time::sleep(SETTLE).await;
    scheduler_a.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S * 5));
    scheduler_a.do_try_match_for_test().await?;
    assert_eq!(waiting.as_state().await?.0.stage, ActionStage::Queued);

    // B's GPU worker leaves. Its next publish carries no GPU shape, so A
    // stops deferring to it.
    scheduler_b
        .remove_worker(&WorkerId("gpu".to_string()))
        .await?;
    tokio::time::sleep(SETTLE).await;
    scheduler_a.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S));
    // Let the exchange re-publish at the advanced clock so the peer census is
    // fresh again: the staleness guard fails open on a census not refreshed
    // within a record TTL, and jumping the mock clock without an exchange
    // running looks exactly like a stalled exchange task.
    tokio::time::sleep(SETTLE).await;
    scheduler_a.do_try_match_for_test().await?;

    let (state, _origin_metadata) = waiting.as_state().await?;
    let ActionStage::Completed(result) = &state.stage else {
        panic!("expected the action to be failed, got {:?}", state.stage);
    };
    let err = result.error.as_ref().expect("an error");
    assert_eq!(err.code, Code::FailedPrecondition);
    assert!(
        err.message_string()
            .contains("'gpu_count' requested 1, largest worker total 0"),
        "{}",
        err.message_string()
    );
    Ok(())
}
