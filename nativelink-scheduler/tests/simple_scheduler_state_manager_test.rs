use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use futures::StreamExt;
use mock_instant::thread_local::MockClock;
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::AwaitedAction;
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler_state_manager::SimpleSchedulerStateManager;
use nativelink_scheduler::worker_registry::WorkerRegistry;
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionStage, ActionState, ActionUniqueKey, ActionUniqueQualifier,
    OperationId, WorkerId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::{
    ClientStateManager, MatchingEngineStateManager, OperationFilter, OperationStageFlags,
    UpdateOperationType, WorkerStateManager,
};
use tokio::sync::Notify;

#[nativelink_test]
async fn drops_missing_actions() -> Result<(), Error> {
    let task_change_notify = Arc::new(Notify::new());
    let awaited_action_db = memory_awaited_action_db_factory(
        0,
        &task_change_notify.clone(),
        MockInstantWrapped::default,
    );
    let state_manager = SimpleSchedulerStateManager::new(
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Duration::ZERO,
        awaited_action_db,
        SystemTime::now,
        None,
    );
    state_manager
        .update_operation(
            &OperationId::Uuid(uuid::Uuid::parse_str(
                "c458c1f4-136e-486d-b9cd-cea07460cde4",
            )?),
            &WorkerId::default(),
            UpdateOperationType::ExecutionComplete,
        )
        .await
        .unwrap();

    assert!(logs_contain(
        "Unable to update action due to it being missing, probably dropped operation_id=c458c1f4-136e-486d-b9cd-cea07460cde4"
    ));
    Ok(())
}

const NOW_TIME: u64 = 10_000;
const WORKER_TIMEOUT: Duration = Duration::from_mins(2);
const MAX_EXECUTING: Duration = Duration::from_mins(15);

fn make_system_time(add_time: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(NOW_TIME + add_time))
        .unwrap()
}

fn action_info(started: SystemTime) -> ActionInfo {
    let action_digest = DigestInfo::zero_digest();
    ActionInfo {
        command_digest: action_digest,
        input_root_digest: action_digest,
        timeout: Duration::ZERO,
        platform_properties: HashMap::default(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: started,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            instance_name: "main".to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    }
}

/// An action already assigned to `worker_id` and executing since `started`.
fn executing_action(worker_id: &WorkerId, started: SystemTime) -> AwaitedAction {
    let action_info = action_info(started);
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
        MAX_EXECUTING,
        memory_awaited_action_db_factory(0, &task_change_notify, MockInstantWrapped::default),
        MockInstantWrapped::default,
        Some(registry),
    )
}

/// Same as above but with `max_action_executing_timeout_s` disabled, which is
/// its default.
fn state_manager_no_executing_ceiling(
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

/// An orphan must still be reaped when `max_action_executing_timeout_s` is
/// disabled, which is its default.
///
/// An HPA scaling a replica down leaves its workers' actions Executing in
/// shared state, naming worker ids no surviving instance recognises. If the
/// only ceiling for Unknown were the executing timeout, the default config
/// would never reap them and every scale-down would leak actions until the
/// client gave up.
#[nativelink_test]
async fn reaps_an_orphan_even_with_the_executing_ceiling_disabled() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let action = executing_action(
        &WorkerId::from(String::from("owner-was-scaled-down")),
        make_system_time(0),
    );
    let state_mgr = state_manager_no_executing_ceiling(Arc::new(WorkerRegistry::new()));

    // Past worker_timeout_s: still must not fire, this could be a healthy
    // action on a peer's worker.
    MockClock::advance(WORKER_TIMEOUT + Duration::from_mins(1));
    assert!(
        !state_mgr.should_timeout_operation(&action).await,
        "must not reap at worker_timeout_s just because the ceiling is disabled"
    );

    // Past the orphan backstop: now it has to go, or it never will.
    MockClock::advance(Duration::from_mins(61));
    assert!(
        state_mgr.should_timeout_operation(&action).await,
        "an orphan must be reaped by the backstop when no ceiling is configured"
    );
    Ok(())
}

/// An action on a worker connected to a DIFFERENT scheduler instance must not
/// be timed out here. This instance never sees that worker's heartbeats, so
/// the action's timestamp looks frozen however healthy it is.
#[nativelink_test]
async fn does_not_time_out_a_peer_instances_worker() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    // Deliberately not registered here: it belongs to another instance.
    let action = executing_action(
        &WorkerId::from(String::from("owned-by-another-scheduler")),
        make_system_time(0),
    );
    let state_mgr = state_manager(Arc::new(WorkerRegistry::new()));

    MockClock::advance(WORKER_TIMEOUT + Duration::from_mins(1));
    assert!(
        !state_mgr.should_timeout_operation(&action).await,
        "must not time out an action on a peer instance's worker"
    );
    Ok(())
}

/// A worker registered here that stopped heartbeating is ours and looks dead,
/// so the short timeout must still fire.
#[nativelink_test]
async fn still_times_out_our_own_dead_worker() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let worker_id = WorkerId::from(String::from("connected-to-us"));
    let action = executing_action(&worker_id, make_system_time(0));

    let registry = Arc::new(WorkerRegistry::new());
    registry
        .register_worker(&worker_id, make_system_time(0))
        .await;
    let state_mgr = state_manager(registry);

    MockClock::advance(WORKER_TIMEOUT + Duration::from_mins(1));
    assert!(
        state_mgr.should_timeout_operation(&action).await,
        "a registered worker that stopped heartbeating must time out"
    );
    Ok(())
}

/// A registered, heartbeating worker is bounded only by the executing ceiling.
#[nativelink_test]
async fn does_not_time_out_a_live_worker_mid_action() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let worker_id = WorkerId::from(String::from("connected-to-us"));
    let action = executing_action(&worker_id, make_system_time(0));

    let elapsed = WORKER_TIMEOUT + Duration::from_mins(1);
    let registry = Arc::new(WorkerRegistry::new());
    registry
        .update_worker_heartbeat(&worker_id, make_system_time(elapsed.as_secs()))
        .await;
    let state_mgr = state_manager(registry);

    MockClock::advance(elapsed);
    assert!(
        !state_mgr.should_timeout_operation(&action).await,
        "a heartbeating worker's action must survive past worker_timeout_s"
    );
    Ok(())
}

/// A heartbeating worker stuck on one action past the executing ceiling must
/// still be timed out, which is what `max_action_executing_timeout_s` is for.
#[nativelink_test]
async fn times_out_a_live_but_stuck_worker() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let worker_id = WorkerId::from(String::from("alive-but-wedged"));
    let action = executing_action(&worker_id, make_system_time(0));

    let elapsed = MAX_EXECUTING + Duration::from_mins(1);
    let registry = Arc::new(WorkerRegistry::new());
    registry
        .update_worker_heartbeat(&worker_id, make_system_time(elapsed.as_secs()))
        .await;
    let state_mgr = state_manager(registry);

    MockClock::advance(elapsed);
    assert!(
        state_mgr.should_timeout_operation(&action).await,
        "a worker stuck past max_action_executing_timeout_s must time out"
    );
    Ok(())
}

/// An orphan whose owning instance died permanently still gets reaped, on the
/// longer ceiling.
#[nativelink_test]
async fn eventually_times_out_an_orphan() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let action = executing_action(
        &WorkerId::from(String::from("owner-died-permanently")),
        make_system_time(0),
    );
    let state_mgr = state_manager(Arc::new(WorkerRegistry::new()));

    MockClock::advance(MAX_EXECUTING + Duration::from_mins(1));
    assert!(
        state_mgr.should_timeout_operation(&action).await,
        "an orphaned action must still be reaped on max_action_executing_timeout_s"
    );
    Ok(())
}

/// The worker scheduler asks this to decide whether a worker should be told
/// to kill an operation, so it must be false for every way an operation can
/// leave a worker: requeued, reassigned, finished, or gone.
#[nativelink_test]
async fn is_executing_on_worker_follows_the_assignment() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let state_mgr = state_manager(Arc::new(WorkerRegistry::new()));
    let worker_id = WorkerId::from(String::from("worker"));
    let other_worker_id = WorkerId::from(String::from("other-worker"));

    let _client_listener = state_mgr
        .add_action(
            OperationId::default(),
            Arc::new(action_info(make_system_time(0))),
        )
        .await?;
    let operation_id = MatchingEngineStateManager::filter_operations(
        state_mgr.as_ref(),
        OperationFilter {
            stages: OperationStageFlags::Queued,
            ..Default::default()
        },
    )
    .await?
    .next()
    .await
    .expect("the queued operation")
    .as_state()
    .await?
    .0
    .client_operation_id
    .clone();

    // Queued: on nobody.
    assert!(
        !state_mgr
            .is_executing_on_worker(&operation_id, &worker_id)
            .await?
    );

    state_mgr
        .assign_operation(&operation_id, Ok(&worker_id))
        .await?;
    assert!(
        state_mgr
            .is_executing_on_worker(&operation_id, &worker_id)
            .await?
    );
    assert!(
        !state_mgr
            .is_executing_on_worker(&operation_id, &other_worker_id)
            .await?
    );

    // Requeued (a timeout), then picked up by another worker.
    state_mgr
        .assign_operation(
            &operation_id,
            Err(make_err!(Code::DeadlineExceeded, "timed out")),
        )
        .await?;
    assert!(
        !state_mgr
            .is_executing_on_worker(&operation_id, &worker_id)
            .await?
    );
    state_mgr
        .assign_operation(&operation_id, Ok(&other_worker_id))
        .await?;
    assert!(
        !state_mgr
            .is_executing_on_worker(&operation_id, &worker_id)
            .await?
    );
    assert!(
        state_mgr
            .is_executing_on_worker(&operation_id, &other_worker_id)
            .await?
    );

    // Finished.
    state_mgr
        .update_operation(
            &operation_id,
            &other_worker_id,
            UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                ActionResult::default(),
            )),
        )
        .await?;
    assert!(
        !state_mgr
            .is_executing_on_worker(&operation_id, &other_worker_id)
            .await?
    );

    // Never existed.
    assert!(
        !state_mgr
            .is_executing_on_worker(&OperationId::default(), &worker_id)
            .await?
    );
    Ok(())
}
