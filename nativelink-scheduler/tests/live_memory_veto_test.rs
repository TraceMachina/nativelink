use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::{PropertyType, SimpleSpec};
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    UpdateForWorker, WorkerLoad, update_for_worker,
};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::ClientStateManager;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW_TIME: u64 = 10000;

fn make_scheduler(spec: &SimpleSpec) -> Arc<SimpleScheduler> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
        spec,
        memory_awaited_action_db_factory(0, &task_change_notify, MockInstantWrapped::default),
        || async move {},
        task_change_notify,
        MockInstantWrapped::default,
        None,
    );
    scheduler
}

async fn add_worker(
    scheduler: &SimpleScheduler,
    name: &str,
    properties: HashMap<String, PlatformPropertyValue>,
) -> Result<mpsc::UnboundedReceiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let worker = Worker::new(
        WorkerId(name.to_string()),
        PlatformProperties::new(properties),
        tx,
        NOW_TIME,
        /* max_inflight_tasks */ 4,
    );
    scheduler
        .add_worker(worker)
        .await
        .err_tip(|| "Failed to add worker")?;
    tokio::task::yield_now().await;
    // The connection result comes first; drop it so only dispatches remain.
    let first = rx.try_recv().expect("worker got no connection result");
    assert!(matches!(
        first.update,
        Some(update_for_worker::Update::ConnectionResult(_))
    ));
    Ok(rx)
}

async fn add_action(
    scheduler: &SimpleScheduler,
    digest_byte: u8,
    platform_properties: HashMap<String, String>,
) -> Result<(), Error> {
    let mut action_info = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([digest_byte; 32], 512),
    );
    Arc::make_mut(&mut action_info).platform_properties = platform_properties;
    scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;
    Ok(())
}

fn dispatched(rx: &mut mpsc::UnboundedReceiver<UpdateForWorker>) -> usize {
    let mut count = 0;
    while let Ok(msg) = rx.try_recv() {
        if matches!(msg.update, Some(update_for_worker::Update::StartAction(_))) {
            count += 1;
        }
    }
    count
}

fn memory_worker(kb: u64) -> HashMap<String, PlatformPropertyValue> {
    HashMap::from([("memory_kb".to_string(), PlatformPropertyValue::Minimum(kb))])
}

fn memory_action(kb: u64) -> HashMap<String, String> {
    HashMap::from([("memory_kb".to_string(), kb.to_string())])
}

/// Two workers with the same advertised memory. One reports on its keepalive
/// that it has almost nothing left; with the veto on, an action asking for
/// more than that goes to the other. A worker that never reports is never
/// vetoed, and a keepalive without a load keeps the last report.
#[nativelink_test]
async fn live_memory_veto_skips_a_worker_that_reports_no_room() -> Result<(), Error> {
    let scheduler = make_scheduler(&SimpleSpec {
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
        live_memory_veto: Some("memory_kb".to_string()),
        ..SimpleSpec::default()
    });
    let mut full = add_worker(&scheduler, "full", memory_worker(100_000)).await?;
    let mut roomy = add_worker(&scheduler, "roomy", memory_worker(100_000)).await?;
    // The ledger sees 100 GB free on both; the workers say otherwise.
    scheduler
        .worker_keep_alive_received(
            &WorkerId("full".to_string()),
            NOW_TIME + 1,
            Some(WorkerLoad {
                free_memory_kb: 1_000,
            }),
        )
        .await?;
    scheduler
        .worker_keep_alive_received(
            &WorkerId("roomy".to_string()),
            NOW_TIME + 1,
            Some(WorkerLoad {
                free_memory_kb: 50_000,
            }),
        )
        .await?;

    add_action(&scheduler, 1, memory_action(2_000)).await?;
    assert_eq!(
        (dispatched(&mut full), dispatched(&mut roomy)),
        (0, 1),
        "the worker reporting 1 MB free must not get a 2 MB action"
    );

    // An action that fits what `full` reports is fine there; the default
    // strategy sends it to the worker used least recently, which is `full`.
    add_action(&scheduler, 2, memory_action(500)).await?;
    assert_eq!((dispatched(&mut full), dispatched(&mut roomy)), (1, 0));

    // A worker that never reported anything is not vetoed, and a keepalive
    // without a load leaves `full` at its last report.
    let mut silent = add_worker(&scheduler, "silent", memory_worker(100_000)).await?;
    scheduler
        .worker_keep_alive_received(&WorkerId("full".to_string()), NOW_TIME + 2, None)
        .await?;
    add_action(&scheduler, 3, memory_action(2_000)).await?;
    assert_eq!(
        dispatched(&mut full) + dispatched(&mut roomy) + dispatched(&mut silent),
        1
    );
    assert_eq!(
        dispatched(&mut full),
        0,
        "a keepalive without a load keeps the last report"
    );
    Ok(())
}

/// Without `live_memory_veto` the report is recorded and ignored: the
/// ledger alone decides, as before.
#[nativelink_test]
async fn without_the_veto_a_worker_reporting_no_room_still_gets_actions() -> Result<(), Error> {
    let scheduler = make_scheduler(&SimpleSpec {
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
        ..SimpleSpec::default()
    });
    let mut full = add_worker(&scheduler, "full", memory_worker(100_000)).await?;
    scheduler
        .worker_keep_alive_received(
            &WorkerId("full".to_string()),
            NOW_TIME + 1,
            Some(WorkerLoad {
                free_memory_kb: 1_000,
            }),
        )
        .await?;
    add_action(&scheduler, 1, memory_action(2_000)).await?;
    assert_eq!(dispatched(&mut full), 1);
    Ok(())
}
