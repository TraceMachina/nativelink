// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;
use tonic::{Request, Response, Status, Streaming};

use proto::google::bytestream::{
    byte_stream_server::ByteStream, byte_stream_server::ByteStreamServer as Server,
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use store::Store;

#[derive(Debug)]
pub struct ByteStreamServer {
    store: Arc<dyn Store>,
}

impl ByteStreamServer {
    pub fn new(store: Arc<dyn Store>) -> Self {
        ByteStreamServer { store: store }
    }

    pub fn into_service(self) -> Server<ByteStreamServer> {
        Server::new(self)
    }
}

#[tonic::async_trait]
impl ByteStream for ByteStreamServer {
    type ReadStream =
        Pin<Box<dyn Stream<Item = Result<ReadResponse, Status>> + Send + Sync + 'static>>;
    async fn read(
        &self,
        _grpc_request: Request<ReadRequest>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        println!("read {:?}", _grpc_request);
        Err(Status::unimplemented(""))
    }

    async fn write(
        &self,
        _grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        println!("write {:?}", _grpc_request);
        Err(Status::unimplemented(""))
    }

    async fn query_write_status(
        &self,
        _grpc_request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        println!("query_write_status {:?}", _grpc_request);
        Err(Status::unimplemented(""))
    }
}
