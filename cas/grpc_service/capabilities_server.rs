// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use tonic::{Request, Response, Status};

use proto::build::bazel::remote::execution::v2::{
    capabilities_server::Capabilities, capabilities_server::CapabilitiesServer as Server,
    GetCapabilitiesRequest, ServerCapabilities,
};

#[derive(Debug, Default)]
pub struct CapabilitiesServer {}

impl CapabilitiesServer {
    pub fn into_service(self) -> Server<CapabilitiesServer> {
        Server::new(self)
    }
}

#[tonic::async_trait]
impl Capabilities for CapabilitiesServer {
    async fn get_capabilities(
        &self,
        _request: Request<GetCapabilitiesRequest>,
    ) -> Result<Response<ServerCapabilities>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
}
