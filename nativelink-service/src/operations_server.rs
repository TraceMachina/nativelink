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

use futures::StreamExt;
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

    async fn list_operations_inner(
        &self,
        scheduler: &Arc<dyn ClientStateManager>,
        page_size: usize,
        page_uuid: Option<Uuid>,
        filter: OperationFilter,
    ) -> Result<ListOperationsResponse, Error> {
        let mut cache = self.cache.lock().await;
        let mut action_state_results = if let Some(uuid) = page_uuid {
            cache.pop(&uuid).ok_or_else(|| {
                Error::new(
                    Code::NotFound,
                    format!("Couldn't find page with token {}", uuid),
                )
            })?
        } else {
            scheduler.filter_operations(filter).await?.collect().await
        };

        let rest = action_state_results.split_off(page_size.min(action_state_results.len()));
        let next_page_token = if !rest.is_empty() {
            let next_page_uuid = Uuid::new_v4();
            let token = next_page_uuid.to_string();

            cache.push(next_page_uuid, rest);
            token
        } else {
            NO_MORE_PAGES_TOKEN.to_string()
        };

        drop(cache);

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
}

#[tonic::async_trait]
impl Operations for OpsServer {
    /// Lists operations that match the specified filter in the request. If the
    /// server doesn't support this method, it returns `UNIMPLEMENTED`.
    ///
    /// NOTE: the `name` binding allows API services to override the binding
    /// to use different resource name schemes, such as `users/*/operations`. To
    /// override the binding, API services can add a binding such as
    /// `"/v1/{name=users/*}/operations"` to their service configuration.
    /// For backwards compatibility, the default name includes the operations
    /// collection id, however overriding users must ensure the name binding
    /// is the parent resource, without the operations collection id.
    async fn list_operations(
        &self,
        request: Request<ListOperationsRequest>,
    ) -> Result<Response<ListOperationsResponse>, Status> {
        let ListOperationsRequest {
            name,
            filter: filter_string,
            page_size,
            page_token,
        } = request.into_inner();

        let normalized_page_size = if page_size < 0 || page_size > LIST_OPERATIONS_MAXIMUM_PAGE_SIZE
        {
            return Err(Status::out_of_range(format!(
                "page size {} out of range 0..=100",
                page_size
            )));
        } else if page_size == 0 {
            LIST_OPERATIONS_DEFAULT_PAGE_SIZE
        } else {
            page_size
                .try_into()
                .expect("a positive number between 0-100 to fit in u32")
        };

        let Some(scheduler) = self
            .schedulers
            .iter()
            .find_map(|(scheduler_name, scheduler)| {
                let n = scheduler_name.len();
                if name.starts_with(scheduler_name.as_str())
                    && name.as_bytes().get(n).is_some_and(|b| *b == b'/')
                {
                    Some(scheduler)
                } else {
                    None
                }
            })
        else {
            return Err(Status::not_found(format!(
                "couldn't find a scheduler named {}",
                &name
            )));
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
            .list_operations_inner(scheduler, normalized_page_size, page_uuid, filter)
            .await?;

        Ok(Response::new(message))
    }

    /// Gets the latest state of a long-running operation.  Clients can use this
    /// method to poll the operation result at intervals as recommended by the API
    /// service.
    async fn get_operation(
        &self,
        request: Request<GetOperationRequest>,
    ) -> Result<Response<Operation>, Status> {
        let GetOperationRequest { name } = request.into_inner();
        let message = self.get_operation_inner(OperationId::String(name)).await?;
        Ok(Response::new(message))
    }
    /// Deletes a long-running operation. This method indicates that the client is
    /// no longer interested in the operation result. It does not cancel the
    /// operation. If the server doesn't support this method, it returns
    /// `google.rpc.Code.UNIMPLEMENTED`.
    async fn delete_operation(
        &self,
        request: Request<DeleteOperationRequest>,
    ) -> Result<Response<()>, Status> {
        Err(Status::unimplemented("unimplemented"))
    }
    /// Starts asynchronous cancellation on a long-running operation.  The server
    /// makes a best effort to cancel the operation, but success is not
    /// guaranteed.  If the server doesn't support this method, it returns
    /// `google.rpc.Code.UNIMPLEMENTED`.  Clients can use
    /// [Operations.GetOperation][google.longrunning.Operations.GetOperation] or
    /// other methods to check whether the cancellation succeeded or whether the
    /// operation completed despite cancellation. On successful cancellation,
    /// the operation is not deleted; instead, it becomes an operation with
    /// an [Operation.error][google.longrunning.Operation.error] value with a [google.rpc.Status.code][google.rpc.Status.code] of 1,
    /// corresponding to `Code.CANCELLED`.
    async fn cancel_operation(
        &self,
        request: Request<CancelOperationRequest>,
    ) -> Result<Response<()>, Status> {
        Err(Status::unimplemented("unimplemented"))
    }
    /// Waits for the specified long-running operation until it is done or reaches
    /// at most a specified timeout, returning the latest state.  If the operation
    /// is already done, the latest state is immediately returned.  If the timeout
    /// specified is greater than the default HTTP/RPC timeout, the HTTP/RPC
    /// timeout is used.  If the server does not support this method, it returns
    /// `google.rpc.Code.UNIMPLEMENTED`.
    /// Note that this method is on a best-effort basis.  It may return the latest
    /// state before the specified timeout (including immediately), meaning even an
    /// immediate response is no guarantee that the operation is done.
    async fn wait_operation(
        &self,
        request: Request<WaitOperationRequest>,
    ) -> Result<Response<Operation>, Status> {
        Err(Status::unimplemented("todo"))
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
