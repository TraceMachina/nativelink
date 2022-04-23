// Copyright 2022 Nathan (Blaise) Bruer.  All rights reserved.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Weak};

use fast_async_mutex::mutex::Mutex;

use ac_utils::get_and_decode_digest;
use action_messages::ActionInfo;
use async_trait::async_trait;
use common::DigestInfo;
use error::{Error, ResultExt};
use proto::build::bazel::remote::execution::v2::Action;
use proto::com::github::allada::turbo_cache::remote_execution::{ExecuteFinishedResult, StartExecute};
use store::Store;

#[async_trait]
pub trait RunningAction: Sync + Send + Sized + Unpin + 'static {
    /// Anything that needs to execute before the actions is actually executed should happen here.
    async fn prepare_action(self: Arc<Self>) -> Result<Arc<Self>, Error>;

    /// Actually perform the execution of the action.
    async fn execute(self: Arc<Self>) -> Result<Arc<Self>, Error>;

    /// Any uploading, processing or analyzing of the results should happen here.
    async fn upload_results(self: Arc<Self>) -> Result<Arc<Self>, Error>;

    /// Cleanup any residual files, handles or other junk resulting from running the action.
    async fn cleanup(self: Arc<Self>) -> Result<Arc<Self>, Error>;

    /// Returns the final result. As a general rule this action should be thought of as
    /// a consumption of `self`, meaning once a return happens here the lifetime of `Self`
    /// is over and any action performed on it after this call is undefined behavior.
    async fn get_finished_result(self: Arc<Self>) -> Result<ExecuteFinishedResult, Error>;
}

pub struct RunningActionImpl {}

impl RunningActionImpl {
    fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl RunningAction for RunningActionImpl {
    async fn prepare_action(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        unimplemented!();
    }

    async fn execute(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        unimplemented!();
    }

    async fn upload_results(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        unimplemented!();
    }

    async fn cleanup(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        unimplemented!();
    }

    async fn get_finished_result(self: Arc<Self>) -> Result<ExecuteFinishedResult, Error> {
        unimplemented!();
    }
}

#[async_trait]
pub trait RunningActionsManager: Sync + Send + Sized + Unpin + 'static {
    type RunningAction: RunningAction;

    async fn create_and_add_action(
        self: Arc<Self>,
        start_execute: StartExecute,
    ) -> Result<Arc<Self::RunningAction>, Error>;
}

type ActionId = [u8; 32];

/// Holds state info about what is being executed and the interface for interacting
/// with actions while they are running.
pub struct RunningActionsManagerImpl {
    cas_store: Pin<Arc<dyn Store>>,
    running_actions: Mutex<HashMap<ActionId, Weak<RunningActionImpl>>>,
}

impl RunningActionsManagerImpl {
    pub fn new(cas_store: Arc<dyn Store>) -> Self {
        Self {
            cas_store: Pin::new(cas_store),
            running_actions: Mutex::new(HashMap::new()),
        }
    }

    async fn create_action_info(&self, start_execute: StartExecute) -> Result<ActionInfo, Error> {
        let execute_request = start_execute
            .execute_request
            .err_tip(|| "Expected execute_request to exist in StartExecute")?;
        let action_digest: DigestInfo = execute_request
            .action_digest
            .clone()
            .err_tip(|| "Expected action_digest to exist on StartExecute")?
            .try_into()?;
        let action = get_and_decode_digest::<Action>(self.cas_store.as_ref(), &action_digest)
            .await
            .err_tip(|| "During start_action")?;
        Ok(
            ActionInfo::try_from_action_and_execute_request_with_salt(execute_request, action, start_execute.salt)
                .err_tip(|| "Could not create ActionInfo in create_and_add_action()")?,
        )
    }
}

#[async_trait]
impl RunningActionsManager for RunningActionsManagerImpl {
    type RunningAction = RunningActionImpl;

    async fn create_and_add_action(
        self: Arc<Self>,
        start_execute: StartExecute,
    ) -> Result<Arc<RunningActionImpl>, Error> {
        let action_info = self.create_action_info(start_execute).await?;
        let action_id = action_info.unique_qualifier.get_hash();
        let running_action = Arc::new(RunningActionImpl::new());
        {
            let mut running_actions = self.running_actions.lock().await;
            running_actions.insert(action_id, Arc::downgrade(&running_action));
        }
        Ok(running_action)
    }
}
