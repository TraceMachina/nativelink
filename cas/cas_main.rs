// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use tonic::transport::Server;

use cas_server::{CasServer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse()?;

    Server::builder()
        .add_service(CasServer::default().into_service())
        .serve(addr)
        .await?;

    Ok(())
}
