// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::pin::Pin;

use futures::Stream;
use tonic::{Request, Response, Status};

use proto::build::bazel::remote::execution::v2::{
    execution_server::Execution, execution_server::ExecutionServer as Server, ExecuteRequest, WaitExecutionRequest,
};
use proto::google::longrunning::Operation;

#[derive(Debug, Default)]
pub struct ExecutionServer {}

impl ExecutionServer {
    pub fn into_service(self) -> Server<ExecutionServer> {
        Server::new(self)
    }
}

#[tonic::async_trait]
impl Execution for ExecutionServer {
    type ExecuteStream = Pin<Box<dyn Stream<Item = Result<Operation, Status>> + Send + Sync + 'static>>;
    async fn execute(&self, _request: Request<ExecuteRequest>) -> Result<Response<Self::ExecuteStream>, Status> {
        use stdext::function_name;
        let output = format!("{} not yet implemented", function_name!());
        println!("{}", output);
        Err(Status::unimplemented(output))
    }

    type WaitExecutionStream = Pin<Box<dyn Stream<Item = Result<Operation, Status>> + Send + Sync + 'static>>;
    async fn wait_execution(
        &self,
        _request: Request<WaitExecutionRequest>,
    ) -> Result<Response<Self::WaitExecutionStream>, Status> {
        use stdext::function_name;
        let output = format!("{} not yet implemented", function_name!());
        println!("{}", output);
        Err(Status::unimplemented(output))
    }
}
