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

//! A scheduler replica judges a worker connected to another replica by the
//! lease that replica renews in the shared store.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::SimpleSpec;
use nativelink_config::stores::RedisSpec;
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_redis_tester::FakeRedisBackend;
use nativelink_scheduler::awaited_action_db::AwaitedAction;
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::simple_scheduler_state_manager::SimpleSchedulerStateManager;
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_registry::{WorkerLeases, WorkerLiveness, WorkerRegistry};
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_store::redis_store::{RedisStore, RedisSubscriptionManager};
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionState, ActionUniqueKey, ActionUniqueQualifier, OperationId,
    WorkerId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::platform_properties::PlatformProperties;
use nativelink_util::store_trait::SchedulerStore;
use parking_lot::Mutex;
use tokio::sync::{Notify, mpsc};

const NOW_TIME: u64 = 10_000;
const WORKER_TIMEOUT: Duration = Duration::from_mins(2);
/// The one-hour backstop for a worker no replica can vouch for, and a
/// minute more.
const PAST_THE_ORPHAN_BACKSTOP: Duration = Duration::from_mins(61);

fn at(offset: Duration) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(NOW_TIME) + offset
}

fn worker(name: &str) -> WorkerId {
    WorkerId(name.to_string())
}

/// Leases kept in memory, as the shared store would keep them.
#[derive(Debug, Default)]
struct FakeLeases {
    /// Leases a live replica keeps renewing: each reads as renewed now.
    kept: Mutex<HashSet<WorkerId>>,
    /// Leases last renewed at a given time.
    renewed: Mutex<HashMap<WorkerId, SystemTime>>,
    reads: AtomicUsize,
    renewals: Mutex<Vec<Vec<WorkerId>>>,
    fail_reads: AtomicBool,
    fail_renewals: AtomicBool,
}

impl FakeLeases {
    fn holding(worker_ids: &[&str]) -> Arc<Self> {
        let leases = Self::default();
        leases
            .kept
            .lock()
            .extend(worker_ids.iter().map(|name| worker(name)));
        Arc::new(leases)
    }

    fn renewals(&self) -> Vec<Vec<WorkerId>> {
        self.renewals.lock().clone()
    }

    /// Waits for the renewals the registry started in the background.
    async fn until_renewed(&self, count: usize) {
        for _ in 0..1_000 {
            if self.renewals.lock().len() >= count {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("expected {count} renewals, got {:?}", self.renewals());
    }
}

#[async_trait]
impl WorkerLeases for FakeLeases {
    async fn renew(
        &self,
        _scheduler_id: &str,
        worker_ids: &[WorkerId],
        now: SystemTime,
        _ttl: Duration,
    ) -> Result<(), Error> {
        let mut renewed = worker_ids.to_vec();
        renewed.sort_by(|a, b| a.0.cmp(&b.0));
        self.renewals.lock().push(renewed);
        if self.fail_renewals.load(Ordering::Acquire) {
            return Err(make_err!(Code::Unavailable, "store unavailable"));
        }
        self.renewed
            .lock()
            .extend(worker_ids.iter().map(|worker_id| (worker_id.clone(), now)));
        Ok(())
    }

    async fn renewed_at(&self, worker_id: &WorkerId) -> Result<Option<SystemTime>, Error> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        if self.fail_reads.load(Ordering::Acquire) {
            return Err(make_err!(Code::Unavailable, "store unavailable"));
        }
        if self.kept.lock().contains(worker_id) {
            return Ok(Some(UNIX_EPOCH + MockClock::time()));
        }
        Ok(self.renewed.lock().get(worker_id).copied())
    }
}

/// A registry on a replica whose peers all renew worker leases.
fn registry_with(leases: Arc<FakeLeases>) -> Arc<WorkerRegistry> {
    let registry = Arc::new(WorkerRegistry::new());
    registry.set_leases(leases);
    registry.set_peers_renew_leases(true);
    registry
}

/// An action executing on `worker_id` since `NOW_TIME`, with no update
/// from it since.
fn executing_on(worker_id: &WorkerId) -> AwaitedAction {
    let started = at(Duration::ZERO);
    let digest = DigestInfo::zero_digest();
    let action_info = ActionInfo {
        command_digest: digest,
        input_root_digest: digest,
        timeout: Duration::ZERO,
        platform_properties: HashMap::default(),
        priority: 0,
        load_timestamp: UNIX_EPOCH,
        insert_timestamp: started,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: "main".to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest,
        }),
    };
    let action_digest = action_info.digest();
    let operation_id = OperationId::default();
    let mut action = AwaitedAction::new(operation_id.clone(), Arc::new(action_info), started);
    action.worker_set_state(
        Arc::new(ActionState {
            stage: ActionStage::Executing,
            client_operation_id: operation_id,
            action_digest,
            last_transition_timestamp: started,
        }),
        started,
    );
    action.set_worker_id(Some(worker_id.clone()), started);
    action
}

/// The executing ceiling is off, its default, so only liveness decides.
fn state_manager(
    registry: Arc<WorkerRegistry>,
) -> Arc<
    SimpleSchedulerStateManager<
        impl nativelink_scheduler::awaited_action_db::AwaitedActionDb,
        MockInstantWrapped,
        fn() -> MockInstantWrapped,
    >,
> {
    let task_change_notify = Arc::new(Notify::new());
    SimpleSchedulerStateManager::new(
        5,
        WORKER_TIMEOUT,
        Duration::from_mins(5),
        Duration::ZERO,
        memory_awaited_action_db_factory(0, &task_change_notify, MockInstantWrapped::default),
        MockInstantWrapped::default,
        Some(registry),
    )
}

/// A replica that died leaves its workers' actions executing in the shared
/// store. A peer used to give them an hour, as it could not tell a dead
/// replica's worker from a live peer's; a lease that stays missing for a
/// lease period tells it.
#[nativelink_test]
async fn an_action_on_a_worker_whose_lease_stays_missing_times_out_a_lease_period_later()
-> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let action = executing_on(&worker("on-a-replica-that-died"));
    let state_mgr = state_manager(registry_with(FakeLeases::holding(&[])));

    MockClock::advance(WORKER_TIMEOUT + Duration::from_secs(1));
    assert!(
        !state_mgr.should_timeout_operation(&action).await,
        "a lease first found missing may be back once the store takes writes again"
    );
    MockClock::advance(WORKER_TIMEOUT);
    assert!(
        state_mgr.should_timeout_operation(&action).await,
        "a worker no replica has held a lease on for a lease period is dead"
    );
    Ok(())
}

/// A worker a peer replica renews the lease of is alive, however long its
/// action runs without an update, as a worker connected here would be.
#[nativelink_test]
async fn an_action_on_a_worker_a_peer_holds_the_lease_of_is_not_timed_out() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let action = executing_on(&worker("on-a-live-peer"));
    let state_mgr = state_manager(registry_with(FakeLeases::holding(&["on-a-live-peer"])));

    MockClock::advance(PAST_THE_ORPHAN_BACKSTOP);
    assert!(
        !state_mgr.should_timeout_operation(&action).await,
        "a worker with a held lease is alive"
    );
    Ok(())
}

/// An older replica renews no leases, so while one runs a missing lease
/// proves nothing and only the backstop applies.
#[nativelink_test]
async fn a_missing_lease_proves_nothing_while_an_older_peer_runs() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let action = executing_on(&worker("on-an-older-peer"));
    let registry = registry_with(FakeLeases::holding(&[]));
    registry.set_peers_renew_leases(false);
    let state_mgr = state_manager(registry);

    for _ in 0..2 {
        MockClock::advance(WORKER_TIMEOUT + Duration::from_secs(1));
        assert!(
            !state_mgr.should_timeout_operation(&action).await,
            "an older peer's worker has no lease and may be alive"
        );
    }
    MockClock::advance(PAST_THE_ORPHAN_BACKSTOP);
    assert!(
        state_mgr.should_timeout_operation(&action).await,
        "the backstop still applies"
    );
    Ok(())
}

#[nativelink_test]
async fn a_lease_that_cannot_be_read_proves_nothing() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let action = executing_on(&worker("unreadable"));
    let leases = FakeLeases::holding(&[]);
    leases.fail_reads.store(true, Ordering::Release);
    let state_mgr = state_manager(registry_with(leases.clone()));

    for _ in 0..2 {
        MockClock::advance(WORKER_TIMEOUT + Duration::from_secs(1));
        assert!(
            !state_mgr.should_timeout_operation(&action).await,
            "a store that cannot be read is no evidence the worker died"
        );
    }
    assert!(leases.reads.load(Ordering::Acquire) > 0);
    Ok(())
}

/// A worker connected here is judged on its heartbeat; its lease is not
/// read.
#[nativelink_test]
async fn a_worker_connected_here_is_judged_on_its_heartbeat() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let leases = FakeLeases::holding(&["here"]);
    let registry = registry_with(leases.clone());
    registry
        .register_worker(&worker("here"), at(Duration::ZERO))
        .await;

    assert_eq!(
        registry
            .fleet_liveness(&worker("here"), WORKER_TIMEOUT, at(WORKER_TIMEOUT * 2))
            .await,
        WorkerLiveness::Stale,
    );
    assert_eq!(leases.reads.load(Ordering::Acquire), 0);
    Ok(())
}

/// A sweep asks about a worker once for each action it holds; one read
/// answers them all for a few seconds.
#[nativelink_test]
async fn one_read_of_a_lease_answers_for_a_few_seconds() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let leases = FakeLeases::holding(&["busy"]);
    let registry = registry_with(leases.clone());
    for _ in 0..100 {
        assert_eq!(
            registry
                .fleet_liveness(&worker("busy"), WORKER_TIMEOUT, at(Duration::ZERO))
                .await,
            WorkerLiveness::Alive,
        );
    }
    assert_eq!(leases.reads.load(Ordering::Acquire), 1);

    leases.kept.lock().clear();
    assert_eq!(
        registry
            .fleet_liveness(&worker("busy"), WORKER_TIMEOUT, at(Duration::from_secs(5)))
            .await,
        WorkerLiveness::Unknown,
        "a lease that lapsed is seen once the earlier read is old"
    );
    assert_eq!(leases.reads.load(Ordering::Acquire), 2);
    Ok(())
}

/// A lease lapses while the store refuses writes, and its replica renews
/// it once they are taken again; only one missing for a whole lease period
/// means a dead worker.
#[nativelink_test]
async fn a_worker_is_dead_only_once_its_lease_has_been_missing_for_a_lease_period()
-> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let leases = FakeLeases::holding(&[]);
    let registry = registry_with(leases.clone());
    let w = worker("w");
    let liveness = |offset: Duration| {
        let registry = registry.clone();
        let w = w.clone();
        async move {
            registry
                .fleet_liveness(&w, WORKER_TIMEOUT, at(offset))
                .await
        }
    };

    // Each check is more than a read's reuse apart, so each reads the
    // store.
    let apart = Duration::from_secs(6);
    assert_eq!(liveness(Duration::ZERO).await, WorkerLiveness::Unknown);
    assert_eq!(
        liveness(WORKER_TIMEOUT.saturating_sub(apart * 2)).await,
        WorkerLiveness::Unknown,
    );

    // Renewed again before a period passed: alive, and a later lapse
    // starts the count again.
    leases
        .renewed
        .lock()
        .insert(w.clone(), at(WORKER_TIMEOUT / 2));
    assert_eq!(
        liveness(WORKER_TIMEOUT.saturating_sub(apart)).await,
        WorkerLiveness::Alive
    );
    leases.renewed.lock().clear();
    let lapse = WORKER_TIMEOUT * 2;
    assert_eq!(liveness(lapse).await, WorkerLiveness::Unknown);
    assert_eq!(
        liveness((lapse + WORKER_TIMEOUT).saturating_sub(apart)).await,
        WorkerLiveness::Unknown,
    );
    assert_eq!(
        liveness(lapse + WORKER_TIMEOUT).await,
        WorkerLiveness::Stale
    );
    Ok(())
}

/// A replica that died between writing a lease and giving it an expiry
/// leaves a key that never goes; its renewal time stops counting.
#[nativelink_test]
async fn a_lease_left_with_no_expiry_stops_counting() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let leases = FakeLeases::holding(&[]);
    leases
        .renewed
        .lock()
        .insert(worker("w"), at(Duration::ZERO));
    let registry = registry_with(leases);

    let three_periods = WORKER_TIMEOUT * 3;
    for (offset, expected) in [
        (
            three_periods.saturating_sub(Duration::from_secs(6)),
            WorkerLiveness::Alive,
        ),
        (three_periods, WorkerLiveness::Unknown),
        (three_periods + WORKER_TIMEOUT, WorkerLiveness::Stale),
    ] {
        assert_eq!(
            registry
                .fleet_liveness(&worker("w"), WORKER_TIMEOUT, at(offset))
                .await,
            expected,
            "at {offset:?}"
        );
    }
    Ok(())
}

/// Every third of the worker timeout, the leases of the workers heard from
/// within it are renewed, so none lapses while its worker lives.
#[nativelink_test]
async fn leases_are_renewed_every_third_of_the_timeout_for_workers_heard_from() -> Result<(), Error>
{
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let leases = FakeLeases::holding(&[]);
    let registry = registry_with(leases.clone());
    registry
        .register_worker(&worker("live"), at(Duration::ZERO))
        .await;
    registry
        .register_worker(&worker("silent"), at(Duration::ZERO))
        .await;
    registry
        .update_worker_heartbeat(&worker("live"), at(WORKER_TIMEOUT))
        .await;

    // "silent" was last heard from a full timeout ago.
    registry
        .renew_leases("here", WORKER_TIMEOUT, at(WORKER_TIMEOUT))
        .await;
    leases.until_renewed(1).await;
    assert_eq!(leases.renewals(), vec![vec![worker("live")]]);

    // The timer calls every second; nothing more until a third has passed.
    let third = WORKER_TIMEOUT / 3;
    registry
        .renew_leases(
            "here",
            WORKER_TIMEOUT,
            at((WORKER_TIMEOUT + third).saturating_sub(Duration::from_secs(1))),
        )
        .await;
    registry
        .renew_leases("here", WORKER_TIMEOUT, at(WORKER_TIMEOUT + third))
        .await;
    leases.until_renewed(2).await;
    assert_eq!(leases.renewals().len(), 2);
    Ok(())
}

#[nativelink_test]
async fn a_failed_renewal_is_tried_again_at_the_next_call() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let leases = FakeLeases::holding(&[]);
    leases.fail_renewals.store(true, Ordering::Release);
    let registry = registry_with(leases.clone());
    registry
        .register_worker(&worker("w"), at(Duration::ZERO))
        .await;

    registry
        .renew_leases("here", WORKER_TIMEOUT, at(Duration::ZERO))
        .await;
    leases.until_renewed(1).await;
    leases.fail_renewals.store(false, Ordering::Release);
    registry
        .renew_leases("here", WORKER_TIMEOUT, at(Duration::from_secs(1)))
        .await;
    leases.until_renewed(2).await;
    assert_eq!(
        leases.renewed.lock().get(&worker("w")),
        Some(&at(Duration::from_secs(1)))
    );
    Ok(())
}

async fn scheduler_on(
    fake_redis_backend: &FakeRedisBackend<RedisSubscriptionManager>,
    port: u16,
    spec: &SimpleSpec,
) -> Result<Arc<SimpleScheduler>, Error> {
    let store = RedisStore::new_standard(RedisSpec {
        addresses: vec![format!("redis://127.0.0.1:{port}")],
        experimental_pub_sub_channel: Some("sub_channel".to_string()),
        ..Default::default()
    })
    .await?;
    fake_redis_backend.set_subscription_manager(store.subscription_manager().await?);
    let db = StoreAwaitedActionDb::new(
        store,
        Arc::new(Notify::new()),
        MockInstantWrapped::default,
        OperationId::default,
        60,
        60,
        false,
    )
    .await?;
    Ok(SimpleScheduler::new_with_fleet_exchange_interval(
        spec,
        db,
        || async move {},
        Arc::new(Notify::new()),
        MockInstantWrapped::default,
        None,
        None,
        Duration::from_millis(50),
    )
    .0)
}

/// Two replicas on one Redis: the one a worker is connected to renews its
/// lease from the worker timeout timer, the other reads it, and once the
/// first stops renewing the lease lapses on the store's clock.
#[nativelink_test]
async fn a_peer_reads_the_lease_its_replica_renews_until_it_lapses() -> Result<(), Error> {
    const TIMEOUT_S: u64 = 30;
    let timeout = Duration::from_secs(TIMEOUT_S);
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let fake_redis_backend: FakeRedisBackend<RedisSubscriptionManager> = FakeRedisBackend::new();
    fake_redis_backend.set_clock(MockClock::time);
    let port = fake_redis_backend.clone().run().await;
    let spec = SimpleSpec {
        worker_timeout_s: TIMEOUT_S,
        client_action_timeout_s: 1_000_000,
        ..Default::default()
    };
    let replica_a = scheduler_on(&fake_redis_backend, port, &spec).await?;
    let replica_b = scheduler_on(&fake_redis_backend, port, &spec).await?;

    let (tx, _rx) = mpsc::channel(64);
    let worker_id = worker("on-a");
    replica_a
        .add_worker(Worker::new(
            worker_id.clone(),
            PlatformProperties::default(),
            tx,
            NOW_TIME,
            1,
        ))
        .await?;
    replica_a.remove_timedout_workers(NOW_TIME).await?;
    let lease_key = format!("wl_{worker_id}");
    tokio::time::timeout(Duration::from_secs(10), async {
        while fake_redis_backend.time_to_live(&lease_key).is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| make_err!(Code::DeadlineExceeded, "the lease was never written"))?;
    assert_eq!(fake_redis_backend.time_to_live(&lease_key), Some(timeout));

    let peer_view = |now: SystemTime| {
        let replica_b = replica_b.clone();
        let worker_id = worker_id.clone();
        async move {
            replica_b
                .worker_registry_for_test()
                .fleet_liveness(&worker_id, timeout, now)
                .await
        }
    };
    assert_eq!(peer_view(at(Duration::ZERO)).await, WorkerLiveness::Alive);

    // Replica A dies: nothing renews the lease any more.
    drop(replica_a);
    MockClock::advance(timeout);
    assert_eq!(peer_view(at(timeout)).await, WorkerLiveness::Unknown);
    // A lease period later, once replica B has read the fleet records, all
    // from replicas that renew leases.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if peer_view(at(timeout * 2)).await == WorkerLiveness::Stale {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| make_err!(Code::DeadlineExceeded, "the peer never saw the lease lapse"))?;
    Ok(())
}
