use core::time::Duration;
use std::sync::Arc;
use std::time::SystemTime;

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler_state_manager::SimpleSchedulerStateManager;
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::{UpdateOperationType, WorkerStateManager};
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
