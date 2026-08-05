use core::time::Duration;
use std::time::SystemTime;

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::worker_registry::WorkerRegistry;
use nativelink_util::action_messages::WorkerId;

#[nativelink_test]
async fn update_worker_heartbeat_logging() -> Result<(), Error> {
    let registry = WorkerRegistry::new();
    registry
        .update_worker_heartbeat(&"foo".to_string().into(), SystemTime::UNIX_EPOCH)
        .await;
    assert!(logs_contain(
        "FLOW: Worker heartbeat updated in registry worker_id=foo now=1970-01-01T00:00:00Z"
    ));
    Ok(())
}

#[nativelink_test]
async fn is_worker_alive_logging() -> Result<(), Error> {
    let registry = WorkerRegistry::new();
    let worker_id: WorkerId = "foo".to_string().into();
    registry
        .update_worker_heartbeat(&worker_id, SystemTime::UNIX_EPOCH)
        .await;
    assert!(
        registry
            .is_worker_alive(&worker_id, Duration::from_secs(10), SystemTime::UNIX_EPOCH)
            .await
    );
    assert!(logs_contain(
        "FLOW: Worker liveness check worker_id=foo last_seen=1970-01-01T00:00:00Z timeout=10s is_alive=true"
    ));
    Ok(())
}
