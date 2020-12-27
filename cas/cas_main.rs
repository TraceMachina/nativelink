// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use tonic::transport::Server;

use ac_server::AcServer;
use capabilities_server::CapabilitiesServer;
use cas_server::CasServer;
use execution_server::ExecutionServer;
use store;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse()?;

    let ac_store = store::create_store(&store::StoreType::Memory);
    let cas_store = store::create_store(&store::StoreType::Memory);

    Server::builder()
        .add_service(AcServer::new(ac_store, cas_store.clone()).into_service())
        .add_service(CasServer::new(cas_store).into_service())
        .add_service(CapabilitiesServer::default().into_service())
        .add_service(ExecutionServer::default().into_service())
        .serve(addr)
        .await?;

    Ok(())
}
