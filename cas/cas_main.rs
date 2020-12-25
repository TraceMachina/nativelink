// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use tonic::transport::Server;

use ac_server::AcServer;
use capabilities_server::CapabilitiesServer;
use cas_server::CasServer;
use execution_server::ExecutionServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse()?;

    Server::builder()
        .add_service(CasServer::default().into_service())
        .add_service(AcServer::default().into_service())
        .add_service(CapabilitiesServer::default().into_service())
        .add_service(ExecutionServer::default().into_service())
        .serve(addr)
        .await?;

    Ok(())
}
