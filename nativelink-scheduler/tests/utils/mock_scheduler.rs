// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::sync::Arc;

use async_trait::async_trait;
use nativelink_error::{make_input_err, Error};
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_util::{
    action_messages::{ActionInfo, OperationId},
    known_platform_property_provider::KnownPlatformPropertyProvider,
    operation_state_manager::{
        ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
    },
};
use tokio::sync::{mpsc, Mutex};

#[allow(clippy::large_enum_variant)]
#[allow(dead_code)] // See https://github.com/rust-lang/rust/issues/46379
enum ActionSchedulerCalls {
    GetGetKnownProperties(String),
    AddAction((OperationId, ActionInfo)),
    FilterOperations(OperationFilter),
}

#[allow(dead_code)] // See https://github.com/rust-lang/rust/issues/46379
enum ActionSchedulerReturns {
    GetGetKnownProperties(Result<Vec<String>, Error>),
    AddAction(Result<Box<dyn ActionStateResult>, Error>),
    FilterOperations(Result<ActionStateResultStream<'static>, Error>),
}

#[derive(MetricsComponent)]
pub struct MockActionScheduler {
    rx_call: Mutex<mpsc::UnboundedReceiver<ActionSchedulerCalls>>,
    tx_call: mpsc::UnboundedSender<ActionSchedulerCalls>,

    rx_resp: Mutex<mpsc::UnboundedReceiver<ActionSchedulerReturns>>,
    tx_resp: mpsc::UnboundedSender<ActionSchedulerReturns>,
}

impl Default for MockActionScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl MockActionScheduler {
    pub fn new() -> Self {
        let (tx_call, rx_call) = mpsc::unbounded_channel();
        let (tx_resp, rx_resp) = mpsc::unbounded_channel();
        Self {
            rx_call: Mutex::new(rx_call),
            tx_call,
            rx_resp: Mutex::new(rx_resp),
            tx_resp,
        }
    }

    #[allow(dead_code)] // See https://github.com/rust-lang/rust/issues/46379
    pub async fn expect_get_known_properties(&self, result: Result<Vec<String>, Error>) -> String {
        let mut rx_call_lock = self.rx_call.lock().await;
        let ActionSchedulerCalls::GetGetKnownProperties(req) = rx_call_lock
            .recv()
            .await
            .expect("Could not receive msg in mpsc")
        else {
            panic!("Got incorrect call waiting for get_known_properties")
        };
        self.tx_resp
            .send(ActionSchedulerReturns::GetGetKnownProperties(result))
            .map_err(|_| make_input_err!("Could not send request to mpsc"))
            .unwrap();
        req
    }

    pub async fn expect_add_action(
        &self,
        result: Result<Box<dyn ActionStateResult>, Error>,
    ) -> (OperationId, ActionInfo) {
        let mut rx_call_lock = self.rx_call.lock().await;
        let ActionSchedulerCalls::AddAction(req) = rx_call_lock
            .recv()
            .await
            .expect("Could not receive msg in mpsc")
        else {
            panic!("Got incorrect call waiting for get_known_properties")
        };
        self.tx_resp
            .send(ActionSchedulerReturns::AddAction(result))
            .map_err(|_| make_input_err!("Could not send request to mpsc"))
            .unwrap();
        req
    }

    pub async fn expect_filter_operations(
        &self,
        result: Result<ActionStateResultStream<'static>, Error>,
    ) -> OperationFilter {
        let mut rx_call_lock = self.rx_call.lock().await;
        let ActionSchedulerCalls::FilterOperations(req) = rx_call_lock
            .recv()
            .await
            .expect("Could not receive msg in mpsc")
        else {
            panic!("Got incorrect call waiting for find_by_client_operation_id")
        };
        self.tx_resp
            .send(ActionSchedulerReturns::FilterOperations(result))
            .map_err(|_| make_input_err!("Could not send request to mpsc"))
            .unwrap();
        req
    }
}

#[async_trait]
impl KnownPlatformPropertyProvider for MockActionScheduler {
    async fn get_known_properties(&self, instance_name: &str) -> Result<Vec<String>, Error> {
        self.tx_call
            .send(ActionSchedulerCalls::GetGetKnownProperties(
                instance_name.to_string(),
            ))
            .expect("Could not send request to mpsc");
        let mut rx_resp_lock = self.rx_resp.lock().await;
        match rx_resp_lock
            .recv()
            .await
            .expect("Could not receive msg in mpsc")
        {
            ActionSchedulerReturns::GetGetKnownProperties(result) => result,
            _ => panic!("Expected get_known_properties return value"),
        }
    }
}

#[async_trait]
impl ClientStateManager for MockActionScheduler {
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        self.tx_call
            .send(ActionSchedulerCalls::AddAction((
                client_operation_id,
                action_info.as_ref().clone(),
            )))
            .expect("Could not send request to mpsc");
        let mut rx_resp_lock = self.rx_resp.lock().await;
        match rx_resp_lock
            .recv()
            .await
            .expect("Could not receive msg in mpsc")
        {
            ActionSchedulerReturns::AddAction(result) => result,
            _ => panic!("Expected add_action return value"),
        }
    }

    async fn filter_operations<'a>(
        &'a self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'a>, Error> {
        self.tx_call
            .send(ActionSchedulerCalls::FilterOperations(filter))
            .expect("Could not send request to mpsc");
        let mut rx_resp_lock = self.rx_resp.lock().await;
        match rx_resp_lock
            .recv()
            .await
            .expect("Could not receive msg in mpsc")
        {
            ActionSchedulerReturns::FilterOperations(result) => result,
            _ => panic!("Expected find_by_client_operation_id return value"),
        }
    }

    fn as_known_platform_property_provider(&self) -> Option<&dyn KnownPlatformPropertyProvider> {
        Some(self)
    }
}

impl RootMetricsComponent for MockActionScheduler {}
