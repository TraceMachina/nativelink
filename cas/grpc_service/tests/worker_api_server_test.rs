// Copyright 2022 Nathan (Blaise) Bruer.  All rights reserved.

use std::collections::HashMap;
use std::sync::Arc;

use tokio_stream::StreamExt;
use tonic::Request;

use error::ResultExt;
use platform_property_manager::PlatformPropertyManager;
use proto::com::github::allada::turbo_cache::remote_execution::{
    update_for_worker, worker_api_server::WorkerApi, SupportedProperties,
};
use scheduler::Scheduler;
use worker_api_server::WorkerApiServer;

#[cfg(test)]
pub mod connect_worker_tests {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    #[tokio::test]
    pub async fn connect_worker_adds_worker_to_scheduler_test() -> Result<(), Box<dyn std::error::Error>> {
        let platform_properties = HashMap::new();
        let scheduler = Arc::new(Scheduler::new());
        let worker_api_server = WorkerApiServer::new(
            Arc::new(PlatformPropertyManager::new(platform_properties)),
            scheduler.clone(),
        );

        let supported_properties = SupportedProperties::default();
        let mut update_for_worker_stream = worker_api_server
            .connect_worker(Request::new(supported_properties))
            .await?
            .into_inner();

        let maybe_first_message = update_for_worker_stream.next().await;
        assert!(maybe_first_message.is_some(), "Expected first message from stream");
        let first_update = maybe_first_message
            .unwrap()
            .err_tip(|| "Expected success result")?
            .update
            .err_tip(|| "Expected update field to be populated")?;
        let worker_id = match first_update {
            update_for_worker::Update::ConnectionResult(connection_result) => connection_result.worker_id,
            other => unreachable!("Expected ConnectionResult, got {:?}", other),
        };

        const UUID_SIZE: usize = 36;
        assert_eq!(worker_id.len(), UUID_SIZE, "Worker ID should be 36 characters");

        let worker_exists = scheduler.contains_worker_for_test(&worker_id.try_into()?).await;
        assert!(worker_exists, "Expected worker to exist in worker map");

        Ok(())
    }
}
