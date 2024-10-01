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

use std::collections::{HashMap, VecDeque};
use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

use futures::{stream, StreamExt, TryStreamExt};
use lru::LruCache;
use nativelink_error::{Code, Error};
use nativelink_proto::google::longrunning::operation::Result as OperationResult;
use nativelink_proto::google::longrunning::operations_server::{Operations, OperationsServer};
use nativelink_proto::google::longrunning::{
    CancelOperationRequest, DeleteOperationRequest, GetOperationRequest, ListOperationsRequest,
    ListOperationsResponse, Operation, WaitOperationRequest,
};
use nativelink_proto::google::rpc;
use nativelink_util::action_messages::{ActionStage, OperationId};
use nativelink_util::operation_state_manager::{
    ActionStateResult, ClientStateManager, OperationFilter,
};
use prost_types::Any;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status};
use uuid::Uuid;

const LIST_OPERATIONS_MAXIMUM_PAGE_SIZE: i32 = 100;
const LIST_OPERATIONS_DEFAULT_PAGE_SIZE: usize = 50;
const NUM_CACHED_RESPONSES: NonZero<usize> = unsafe { NonZero::new_unchecked(1024) };
const NO_MORE_PAGES_TOKEN: &str = "NO_MORE_PAGES";
const WAIT_OPERATION_DEFAULT_TIMEOUT: prost_types::Duration = prost_types::Duration {
    seconds: 20,
    nanos: 0,
};

pub struct OpsServer {
    schedulers: HashMap<String, Arc<dyn ClientStateManager>>,
    cache: Mutex<LruCache<Uuid, VecDeque<Box<dyn ActionStateResult>>>>,
}

impl Clone for OpsServer {
    fn clone(&self) -> Self {
        Self::new(self.schedulers.clone())
    }

    fn clone_from(&mut self, source: &Self) {
        self.schedulers.clone_from(&source.schedulers);
    }
}

impl OpsServer {
    pub fn new(schedulers: HashMap<String, Arc<dyn ClientStateManager>>) -> Self {
        Self {
            schedulers,
            cache: Mutex::new(LruCache::new(NUM_CACHED_RESPONSES)),
        }
    }

    pub fn into_service(self) -> OperationsServer<OpsServer> {
        OperationsServer::new(self)
    }

    /// List operations matching a given filter.
    async fn list_operations_inner(
        &self,
        page_size: usize,
        page_uuid: Option<Uuid>,
        filter: OperationFilter,
    ) -> Result<ListOperationsResponse, Error> {
        let mut action_state_results = if let Some(uuid) = page_uuid {
            self.cache.lock().await.pop(&uuid).ok_or_else(|| {
                Error::new(
                    Code::NotFound,
                    format!("Couldn't find page with token {uuid}"),
                )
            })?
        } else {
            let schedulers = self.schedulers.values().cloned();
            stream::iter(schedulers)
                .then(|scheduler| {
                    let filter = filter.clone();
                    async move {
                        let operations = scheduler.filter_operations(filter).await?;
                        Ok(operations.collect::<Vec<_>>().await)
                    }
                })
                .try_fold(VecDeque::new(), |mut queue, mut operations| async move {
                    queue.extend(operations.drain(..));
                    Ok::<_, Error>(queue)
                })
                .await?
        };

        let rest = action_state_results.split_off(page_size.min(action_state_results.len()));
        let next_page_token = if !rest.is_empty() {
            let next_page_uuid = Uuid::new_v4();
            let token = next_page_uuid.to_string();

            self.cache.lock().await.push(next_page_uuid, rest);
            token
        } else {
            NO_MORE_PAGES_TOKEN.to_string()
        };

        let mut out = Vec::with_capacity(action_state_results.len());
        for action_state_result in action_state_results {
            let operation = translate_action_stage_result(action_state_result).await?;
            out.push(operation);
        }

        Ok(ListOperationsResponse {
            operations: out,
            next_page_token,
        })
    }

    /// Get an operation, if it exists.
    async fn get_operation_inner(
        &self,
        client_operation_id: OperationId,
    ) -> Result<Operation, Error> {
        for scheduler in self.schedulers.values() {
            if let Some(action_state_result) = scheduler
                .filter_operations(OperationFilter {
                    client_operation_id: Some(client_operation_id.clone()),
                    ..Default::default()
                })
                .await?
                .next()
                .await
            {
                return translate_action_stage_result(action_state_result).await;
            }
        }

        Err(Error::new(
            Code::NotFound,
            format!(
                "Couldn't find operation with ID {}",
                client_operation_id.into_string()
            ),
        ))
    }

    /// Wait (potentially forever) for an operation to complete.
    async fn wait_operation_inner(&self, operation_id: OperationId) -> Result<Operation, Error> {
        let mut action_state_result_maybe = None;
        for scheduler in self.schedulers.values() {
            if let Some(action_state_result) = scheduler
                .filter_operations(OperationFilter {
                    client_operation_id: Some(operation_id.clone()),
                    ..Default::default()
                })
                .await?
                .next()
                .await
            {
                action_state_result_maybe = Some(action_state_result);
                break;
            }
        }

        let Some(mut action_state_result) = action_state_result_maybe else {
            return Err(Error::new(
                Code::NotFound,
                format!(
                    "Couldn't find operation with ID {}",
                    operation_id.into_string()
                ),
            ));
        };

        let mut state = action_state_result.as_state().await?;
        loop {
            match state.stage {
                ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) => {
                    return translate_action_stage_result(action_state_result).await
                }
                _ => {
                    state = action_state_result.changed().await?;
                }
            }
        }
    }
}

#[tonic::async_trait]
impl Operations for OpsServer {
    async fn list_operations(
        &self,
        request: Request<ListOperationsRequest>,
    ) -> Result<Response<ListOperationsResponse>, Status> {
        let ListOperationsRequest {
            filter: filter_string,
            page_size,
            page_token,
            ..
        } = request.into_inner();

        let normalized_page_size = if !(0..=LIST_OPERATIONS_MAXIMUM_PAGE_SIZE).contains(&page_size)
        {
            return Err(Status::out_of_range(format!(
                "page size {page_size} out of range 0..=100",
            )));
        } else if page_size == 0 {
            LIST_OPERATIONS_DEFAULT_PAGE_SIZE
        } else {
            page_size
                .try_into()
                .expect("a positive number between 0-100 to fit in u32")
        };

        let filter = if filter_string.is_empty() {
            OperationFilter::default()
        } else {
            return Err(Status::unimplemented("filtering not implemented yet"));
        };

        let page_uuid = if page_token.is_empty() {
            None
        } else if page_token == NO_MORE_PAGES_TOKEN {
            return Ok(Response::new(ListOperationsResponse {
                operations: vec![],
                next_page_token: NO_MORE_PAGES_TOKEN.to_string(),
            }));
        } else {
            match page_token.parse() {
                Ok(uuid) => Some(uuid),
                Err(e) => {
                    return Err(Status::invalid_argument(format!(
                        "Invalid page token {page_token}: {e}"
                    )))
                }
            }
        };

        let message = self
            .list_operations_inner(normalized_page_size, page_uuid, filter)
            .await?;

        Ok(Response::new(message))
    }

    async fn get_operation(
        &self,
        request: Request<GetOperationRequest>,
    ) -> Result<Response<Operation>, Status> {
        let GetOperationRequest { name } = request.into_inner();
        let message = self.get_operation_inner(OperationId::String(name)).await?;
        Ok(Response::new(message))
    }

    async fn delete_operation(
        &self,
        _: Request<DeleteOperationRequest>,
    ) -> Result<Response<()>, Status> {
        Err(Status::unimplemented("UNIMPLEMENTED"))
    }

    async fn cancel_operation(
        &self,
        _: Request<CancelOperationRequest>,
    ) -> Result<Response<()>, Status> {
        Err(Status::unimplemented("UNIMPLEMENTED"))
    }

    async fn wait_operation(
        &self,
        request: Request<WaitOperationRequest>,
    ) -> Result<Response<Operation>, Status> {
        let rpc_timeout: Duration =
            if let Some(grpc_timeout_header) = request.metadata().get("grpc-timeout") {
                grpc_timeout_header
                    .to_str()
                    .map_err(|e| Status::invalid_argument(format!("invalid grpc-timeout: {e}")))?
                    .parse::<prost_types::Duration>()
                    .map_err(|e| Status::invalid_argument(format!("invalid grpc-timeout: {e}")))?
                    .try_into()
                    .map_err(|e| Status::invalid_argument(format!("invalid grpc-timeout: {e}")))?
            } else {
                WAIT_OPERATION_DEFAULT_TIMEOUT
                    .try_into()
                    .expect("a positive timeout to translate")
            };

        let WaitOperationRequest {
            name,
            timeout: message_timeout_maybe,
        } = request.into_inner();

        let message_timeout: Duration = message_timeout_maybe
            .unwrap_or(WAIT_OPERATION_DEFAULT_TIMEOUT)
            .try_into()
            .map_err(|e| Status::invalid_argument(format!("invalid timeout: {e}")))?;

        let timeout = rpc_timeout.min(message_timeout);
        let operation_id = OperationId::String(name);

        let message = tokio::time::timeout(timeout, self.wait_operation_inner(operation_id))
            .await
            .map_err(|_| Status::deadline_exceeded("timeout elapsed"))??;

        Ok(Response::new(message))
    }
}

async fn translate_action_stage_result(
    action_state_result: Box<dyn ActionStateResult>,
) -> Result<Operation, Error> {
    let info = action_state_result.as_action_info().await?;
    let state = action_state_result.as_state().await?;

    let name = info.unique_qualifier.digest().to_string();
    let metadata = None;
    let (done, result) = match &state.stage {
        ActionStage::Completed(action_result) => {
            let result = if action_result.exit_code == 0 {
                OperationResult::Response(Any::default())
            } else {
                OperationResult::Error(rpc::Status {
                    code: action_result.exit_code,
                    message: action_result.message.clone(),
                    details: vec![],
                })
            };

            (true, Some(result))
        }
        ActionStage::CompletedFromCache(cached_action_result) => {
            let result = if cached_action_result.exit_code == 0 {
                OperationResult::Response(Any::default())
            } else {
                OperationResult::Error(rpc::Status {
                    code: cached_action_result.exit_code,
                    message: String::from_utf8_lossy(&cached_action_result.stderr_raw).into_owned(),
                    details: vec![],
                })
            };

            (true, Some(result))
        }
        _ => (false, None),
    };

    Ok(Operation {
        name,
        metadata,
        done,
        result,
    })
}
