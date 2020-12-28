// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::pin::Pin;

use futures_core::Stream;
use tonic::{Request, Response, Status, Streaming};

use proto::google::bytestream::{
    byte_stream_server::ByteStream, byte_stream_server::ByteStreamServer as Server,
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};

#[derive(Debug, Default)]
pub struct ByteStreamServer {}

impl ByteStreamServer {
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
        Err(Status::unimplemented(""))
    }

    async fn write(
        &self,
        _grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        Err(Status::unimplemented(""))
    }

    async fn query_write_status(
        &self,
        _grpc_request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        Err(Status::unimplemented(""))
    }
}
