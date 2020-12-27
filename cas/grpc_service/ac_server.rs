// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use tonic::{Request, Response, Status};

use proto::build::bazel::remote::execution::v2::{
    action_cache_server::ActionCache, action_cache_server::ActionCacheServer as Server,
    ActionResult, GetActionResultRequest, UpdateActionResultRequest,
};
use std::sync::Arc;
use store::Store;

#[derive(Debug)]
pub struct AcServer {
    ac_store: Arc<dyn Store>,
    cas_store: Arc<dyn Store>,
}

impl AcServer {
    pub fn new(ac_store: Arc<dyn Store>, cas_store: Arc<dyn Store>) -> Self {
        AcServer {
            ac_store: ac_store,
            cas_store: cas_store,
        }
    }

    pub fn into_service(self) -> Server<AcServer> {
        Server::new(self)
    }
}

#[tonic::async_trait]
impl ActionCache for AcServer {
    async fn get_action_result(
        &self,
        _grpc_request: Request<GetActionResultRequest>,
    ) -> Result<Response<ActionResult>, Status> {
        use stdext::function_name;
        let output = format!("{} not yet implemented", function_name!());
        println!("{}", output);
        Err(Status::unimplemented(output))
    }

    async fn update_action_result(
        &self,
        _grpc_request: Request<UpdateActionResultRequest>,
    ) -> Result<Response<ActionResult>, Status> {
        use stdext::function_name;
        let output = format!("{} not yet implemented", function_name!());
        println!("{}", output);
        Err(Status::unimplemented(output))
    }
}
