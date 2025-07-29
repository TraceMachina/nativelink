// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use std::collections::HashMap;
use std::sync::Arc;

use nativelink_config::cas_server::{ExecutionConfig, WithInstanceName};
use nativelink_config::stores::{MemorySpec, StoreSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::execution_server::Execution;
use nativelink_proto::build::bazel::remote::execution::v2::{ExecuteRequest, digest_function};
use nativelink_scheduler::mock_scheduler::MockActionScheduler;
use nativelink_service::execution_server::ExecutionServer;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::operation_state_manager::ClientStateManager;
use tonic::Request;

const INSTANCE_NAME: &str = "instance_name";

async fn make_store_manager() -> Result<Arc<StoreManager>, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        "main_cas",
        store_factory(
            &StoreSpec::Memory(MemorySpec::default()),
            &store_manager,
            None,
        )
        .await?,
    );
    Ok(store_manager)
}

fn make_execution_server(store_manager: &StoreManager) -> Result<ExecutionServer, Error> {
    let mock_scheduler = Arc::new(MockActionScheduler::new());
    let mut action_schedulers: HashMap<String, Arc<dyn ClientStateManager>> = HashMap::new();
    action_schedulers.insert("main_scheduler".to_string(), mock_scheduler);
    ExecutionServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: ExecutionConfig {
                cas_store: "main_cas".to_string(),
                scheduler: "main_scheduler".to_string(),
            },
        }],
        &action_schedulers,
        store_manager,
    )
}

#[nativelink_test]
async fn instance_name_fail() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let execution_server = make_execution_server(&store_manager)?;

    let raw_response = execution_server
        .execute(Request::new(ExecuteRequest {
            instance_name: "foo".to_string(),
            digest_function: digest_function::Value::Sha256.into(),
            skip_cache_lookup: false,
            action_digest: None,
            execution_policy: None,
            results_cache_policy: None,
        }))
        .await;

    match raw_response {
        Err(response_err) => {
            assert_eq!(
                response_err.message(),
                "'instance_name' not configured for 'foo' : Failed on execute() command"
            );
        }
        Ok(_) => {
            panic!("Not expecting ok!");
        }
    }
    Ok(())
}
