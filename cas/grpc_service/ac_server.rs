// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use tonic::{Request, Response, Status};

use proto::build::bazel::remote::execution::v2::{
    action_cache_server::ActionCache, action_cache_server::ActionCacheServer as Server,
    ActionResult, GetActionResultRequest, UpdateActionResultRequest,
};

#[derive(Debug, Default)]
pub struct AcServer {}

impl AcServer {
    pub fn into_service(self) -> Server<AcServer> {
        Server::new(self)
    }
}

#[tonic::async_trait]
impl ActionCache for AcServer {
    async fn get_action_result(
        &self,
        _request: Request<GetActionResultRequest>,
    ) -> Result<Response<ActionResult>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn update_action_result(
        &self,
        _request: Request<UpdateActionResultRequest>,
    ) -> Result<Response<ActionResult>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
}
