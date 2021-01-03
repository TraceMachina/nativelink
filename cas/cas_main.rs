// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use tonic::transport::Server;

use ac_server::AcServer;
use bytestream_server::ByteStreamServer;
use capabilities_server::CapabilitiesServer;
use cas_server::CasServer;
use execution_server::ExecutionServer;
use store::{StoreConfig, StoreType};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let addr = "0.0.0.0:50051".parse()?;

    let ac_store = store::create_store(&StoreConfig {
        store_type: StoreType::Memory,
        verify_size: false,
    });
    let cas_store = store::create_store(&StoreConfig {
        store_type: StoreType::Memory,
        verify_size: true,
    });

    Server::builder()
        .add_service(AcServer::new(ac_store, cas_store.clone()).into_service())
        .add_service(CasServer::new(cas_store.clone()).into_service())
        .add_service(CapabilitiesServer::default().into_service())
        .add_service(ExecutionServer::default().into_service())
        .add_service(ByteStreamServer::new(cas_store).into_service())
        .serve(addr)
        .await?;

    Ok(())
}
