// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::BytesMut;
use futures::{future::Either, Stream, StreamExt};
use prost::Message;
use rand::{thread_rng, Rng};
use tokio::time::interval;
use tokio_stream::wrappers::WatchStream;
use tonic::{Request, Response, Status};

use ac_utils::{get_and_decode_digest, ESTIMATED_DIGEST_SIZE};
use action_messages::{ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState};
use common::{log, DigestInfo};
use config::cas_server::{ExecutionConfig, InstanceName};
use error::{make_input_err, Error, ResultExt};
use grpc_store::GrpcStore;
use platform_property_manager::PlatformProperties;
use proto::build::bazel::remote::execution::v2::{
    execution_server::Execution, execution_server::ExecutionServer as Server, Action,
    ActionResult as ProtoActionResult, ExecuteRequest, FindMissingBlobsRequest, GetActionResultRequest,
    UpdateActionResultRequest, WaitExecutionRequest,
};
use proto::google::longrunning::Operation;
use scheduler::{CompleteCallback, Scheduler};
use store::{Store, StoreManager};

/// Default priority remote execution jobs will get when not provided.
const DEFAULT_EXECUTION_PRIORITY: i32 = 0;

struct InstanceInfo {
    scheduler: Arc<Scheduler>,
    cas_store: Arc<dyn Store>,
    ac_store: Option<Arc<dyn Store>>,
}

impl InstanceInfo {
    fn cas_pin<'a>(&'a self) -> Pin<&'a dyn Store> {
        Pin::new(self.cas_store.as_ref())
    }

    async fn build_action_info(
        &self,
        instance_name: String,
        action_digest: DigestInfo,
        action: &Action,
        priority: i32,
    ) -> Result<ActionInfo, Error> {
        let command_digest = DigestInfo::try_from(
            action
                .command_digest
                .clone()
                .err_tip(|| "Expected command_digest to exist")?,
        )
        .err_tip(|| "Could not decode command digest")?;

        let input_root_digest = DigestInfo::try_from(
            action
                .clone()
                .input_root_digest
                .err_tip(|| "Expected input_digest_root")?,
        )?;
        let timeout = action
            .timeout
            .clone()
            .map(|v| Duration::new(v.seconds as u64, v.nanos as u32))
            .unwrap_or(Duration::MAX);

        let mut platform_properties = HashMap::new();
        if let Some(platform) = &action.platform {
            for property in &platform.properties {
                let platform_property = self
                    .scheduler
                    .get_platform_property_manager()
                    .make_prop_value(&property.name, &property.value)
                    .err_tip(|| "Failed to convert platform property in queue_action")?;
                platform_properties.insert(property.name.clone(), platform_property);
            }
        }

        Ok(ActionInfo {
            instance_name,
            command_digest,
            input_root_digest,
            timeout,
            platform_properties: PlatformProperties::new(platform_properties),
            priority,
            load_timestamp: UNIX_EPOCH,
            insert_timestamp: SystemTime::now(),
            unique_qualifier: ActionInfoHashKey {
                digest: action_digest,
                salt: if action.do_not_cache {
                    thread_rng().gen::<u64>()
                } else {
                    0
                },
            },
        })
    }
}

pub struct ExecutionServer {
    instance_infos: HashMap<InstanceName, InstanceInfo>,
}

type ExecuteStream = Pin<Box<dyn Stream<Item = Result<Operation, Status>> + Send + Sync + 'static>>;

impl ExecutionServer {
    pub fn new(
        config: &HashMap<InstanceName, ExecutionConfig>,
        scheduler_map: &HashMap<String, Arc<Scheduler>>,
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut instance_infos = HashMap::with_capacity(config.len());
        for (instance_name, exec_cfg) in config {
            let cas_store = store_manager
                .get_store(&exec_cfg.cas_store)
                .ok_or_else(|| make_input_err!("'cas_store': '{}' does not exist", exec_cfg.cas_store))?;
            let ac_store = match &exec_cfg.ac_store {
                Some(ac_store) => Some(
                    store_manager
                        .get_store(&ac_store)
                        .ok_or_else(|| make_input_err!("'ac_store': '{}' does not exist", ac_store))?,
                ),
                None => None,
            };
            let scheduler = scheduler_map
                .get(&exec_cfg.scheduler)
                .err_tip(|| {
                    format!(
                        "Scheduler needs config for '{}' because it exists in execution",
                        exec_cfg.scheduler
                    )
                })?
                .clone();

            // This will protect us from holding a reference to the scheduler forever in the
            // event our ExecutionServer dies. Our scheduler is a weak ref, so the spawn will
            // eventually see the Arc went away and return.
            let weak_scheduler = Arc::downgrade(&scheduler);
            instance_infos.insert(
                instance_name.to_string(),
                InstanceInfo {
                    scheduler,
                    cas_store,
                    ac_store,
                },
            );
            tokio::spawn(async move {
                let mut ticker = interval(Duration::from_secs(1));
                loop {
                    ticker.tick().await;
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("Error: system time is now behind unix epoch");
                    match weak_scheduler.upgrade() {
                        Some(scheduler) => {
                            if let Err(e) = scheduler.remove_timedout_workers(timestamp.as_secs()).await {
                                log::error!("Error while running remove_timedout_workers : {:?}", e);
                            }
                        }
                        // If we fail to upgrade, our service is probably destroyed, so return.
                        None => return,
                    }
                }
            });
        }
        Ok(Self { instance_infos })
    }

    pub fn into_service(self) -> Server<ExecutionServer> {
        Server::new(self)
    }

    fn send_cached_result(cached_result: ProtoActionResult, digest: DigestInfo) -> Response<ExecuteStream> {
        let operation = ActionState {
            name: format!("{:X}", thread_rng().gen::<u128>()),
            action_digest: digest,
            stage: ActionStage::CompletedFromCache(cached_result),
        }
        .into();
        let result_stream = Box::pin(futures::stream::once(async { Ok(operation) }));
        tonic::Response::new(result_stream)
    }

    fn create_action_save_callback(ac_store_weak: Weak<dyn Store>) -> CompleteCallback {
        Box::new(
            move |action_update: Arc<ActionState>| -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
                Box::pin(
                    if let (ActionStage::Completed(action_result), Some(ac_store)) =
                        (&action_update.stage, ac_store_weak.upgrade())
                    {
                        Either::Left(Self::save_action_in_store(
                            ac_store,
                            action_update.action_digest.clone(),
                            action_result.clone(),
                        ))
                    } else {
                        log::error!(
                            "\x1b[0;31mInstance died before action cache\x1b[0m: {:?}",
                            action_update
                        );
                        Either::Right(futures::future::ready(()))
                    },
                )
            },
        )
    }

    async fn inner_execute(&self, request: Request<ExecuteRequest>) -> Result<Response<ExecuteStream>, Error> {
        let execute_req = request.into_inner();
        let instance_name = execute_req.instance_name;

        let instance_info = self
            .instance_infos
            .get(&instance_name)
            .err_tip(|| "Instance name '{}' not configured")?;

        let digest = DigestInfo::try_from(
            execute_req
                .action_digest
                .err_tip(|| "Expected action_digest to exist")?,
        )
        .err_tip(|| "Failed to unwrap action cache")?;

        let priority = execute_req
            .execution_policy
            .map_or(DEFAULT_EXECUTION_PRIORITY, |p| p.priority);

        // Check to see if the store already has a result for this action.
        if let Some(ac_store) = &instance_info.ac_store {
            if let Some(cached_result) =
                Self::get_action_from_store(ac_store.clone(), instance_info.cas_store.clone(), &digest).await
            {
                return Ok(Self::send_cached_result(cached_result, digest));
            }
        }

        let action = get_and_decode_digest::<Action>(instance_info.cas_pin(), &digest).await?;
        let action_info = instance_info
            .build_action_info(instance_name, digest, &action, priority)
            .await?;

        // Create a callback that will cache the result if it completes.
        let complete_callback = match &instance_info.ac_store {
            Some(ac_store) => Some(Self::create_action_save_callback(Arc::downgrade(&ac_store))),
            None => None,
        };

        let rx = instance_info
            .scheduler
            .add_action(action_info, complete_callback)
            .await
            .err_tip(|| "Failed to schedule task")?;

        let receiver_stream = Box::pin(WatchStream::new(rx).map(|action_update| {
            log::info!("\x1b[0;31mexecute Resp Stream\x1b[0m: {:?}", action_update);
            Ok(action_update.as_ref().clone().into())
        }));
        Ok(tonic::Response::new(receiver_stream))
    }

    async fn get_action_from_store(
        ac_store: Arc<dyn Store>,
        cas_store: Arc<dyn Store>,
        action_digest: &DigestInfo,
    ) -> Option<ProtoActionResult> {
        // If we are a GrpcStore we shortcut here, as this is a special store.
        let any_store = ac_store.clone().as_any();
        let maybe_grpc_store = any_store.downcast_ref::<Arc<GrpcStore>>();
        let result = if let Some(grpc_store) = maybe_grpc_store {
            let action_result_request = GetActionResultRequest {
                instance_name: "".to_string(),
                action_digest: Some(action_digest.into()),
                inline_stdout: false,
                inline_stderr: false,
                inline_output_files: Vec::new(),
            };
            grpc_store
                .get_action_result(Request::new(action_result_request))
                .await
                .map(|response| response.into_inner())
        } else {
            get_and_decode_digest::<ProtoActionResult>(Pin::new(ac_store.as_ref()), &action_digest).await
        };
        if result.is_err() {
            return None;
        }
        let result = result.unwrap();

        // Verify that output_files and output_directories are available in the cas.
        let required_digests = result
            .output_files
            .iter()
            .filter_map(|output_file| output_file.digest.clone())
            .chain(
                result
                    .output_directories
                    .iter()
                    .filter_map(|output_directory| output_directory.tree_digest.clone()),
            )
            .collect();

        // If the CAS is a GrpcStore store we can check all the digests in one message.
        let any_store = cas_store.clone().as_any();
        let maybe_grpc_store = any_store.downcast_ref::<Arc<GrpcStore>>();
        if let Some(grpc_store) = maybe_grpc_store {
            let find_result = grpc_store
                .find_missing_blobs(Request::new(FindMissingBlobsRequest {
                    instance_name: "".to_string(),
                    blob_digests: required_digests,
                }))
                .await;
            if find_result.is_err() || !find_result.unwrap().into_inner().missing_blob_digests.is_empty() {
                return None;
            }
        } else {
            let cas_pin = Pin::new(cas_store.as_ref());
            for digest in required_digests
                .iter()
                .filter_map(|digest| DigestInfo::try_from(digest.clone()).ok())
            {
                if cas_pin.has(digest).await.is_err() {
                    return None;
                }
            }
        };

        // All good, return the cached result.
        Some(result)
    }

    async fn save_action_in_store(ac_store: Arc<dyn Store>, action_digest: DigestInfo, action_result: ActionResult) {
        let mut store_data = BytesMut::with_capacity(ESTIMATED_DIGEST_SIZE);
        let proto_action_result: ProtoActionResult = action_result.into();
        match proto_action_result.encode(&mut store_data) {
            Err(_) => {
                log::error!("\x1b[0;31mError encoding action to save\x1b[0m: {:?}", action_digest);
                return;
            }
            _ => (),
        }
        // If we are a GrpcStore we shortcut here, as this is a special store.
        let any_store = ac_store.clone().as_any();
        let maybe_grpc_store = any_store.downcast_ref::<Arc<GrpcStore>>();
        let result = if let Some(grpc_store) = maybe_grpc_store {
            let update_action_request = UpdateActionResultRequest {
                instance_name: "".to_string(),
                action_digest: Some(action_digest.clone().into()),
                action_result: Some(proto_action_result),
                results_cache_policy: None,
            };
            grpc_store
                .update_action_result(Request::new(update_action_request))
                .await
                .map(|_| ())
        } else {
            Pin::new(ac_store.as_ref())
                .update_oneshot(action_digest.clone(), store_data.freeze())
                .await
        };
        if result.is_err() {
            log::error!("\x1b[0;31mError saving action in store\x1b[0m: {:?}", action_digest);
        }
    }
}

#[tonic::async_trait]
impl Execution for ExecutionServer {
    type ExecuteStream = ExecuteStream;

    async fn execute(&self, grpc_request: Request<ExecuteRequest>) -> Result<Response<ExecuteStream>, Status> {
        // TODO(blaise.bruer) This is a work in progress, remote execution likely won't work yet.
        log::info!("\x1b[0;31mexecute Req\x1b[0m: {:?}", grpc_request.get_ref());
        let now = Instant::now();
        let resp = self
            .inner_execute(grpc_request)
            .await
            .err_tip(|| "Failed on execute() command")
            .map_err(|e| e.into());
        let d = now.elapsed().as_secs_f32();
        if let Err(err) = &resp {
            log::error!("\x1b[0;31mexecute Resp\x1b[0m: {} {:?}", d, err);
        } else {
            log::info!("\x1b[0;31mexecute Resp\x1b[0m: {}", d);
        }
        resp
    }

    type WaitExecutionStream = Pin<Box<dyn Stream<Item = Result<Operation, Status>> + Send + Sync + 'static>>;
    async fn wait_execution(
        &self,
        request: Request<WaitExecutionRequest>,
    ) -> Result<Response<Self::WaitExecutionStream>, Status> {
        use stdext::function_name;
        let output = format!("{} not yet implemented", function_name!());
        println!("{:?}", request);
        Err(Status::unimplemented(output))
    }
}
