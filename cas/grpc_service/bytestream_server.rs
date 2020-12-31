// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tonic::{Request, Response, Status, Streaming};

use proto::google::bytestream::{
    byte_stream_server::ByteStream, byte_stream_server::ByteStreamServer as Server,
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};

use async_fixed_buffer::AsyncFixedBuf;
use macros::{error_if, make_input_err};
use store::Store;

#[derive(Debug)]
pub struct ByteStreamServer {
    store: Arc<dyn Store>,
    max_stream_buffer_size: usize,
}

impl ByteStreamServer {
    pub fn new(store: Arc<dyn Store>) -> Self {
        ByteStreamServer {
            store: store,
            // TODO(allada) Make this configurable.
            // This value was choosen only because it is a common mem page size.
            max_stream_buffer_size: (2 << 20) - 1, // 2MB.
        }
    }

    pub fn into_service(self) -> Server<ByteStreamServer> {
        Server::new(self)
    }

    async fn inner_write(
        &self,
        grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        let mut stream = WriteRequestStreamWrapper::from(grpc_request.into_inner()).await?;

        let raw_buffer = vec![0u8; self.max_stream_buffer_size].into_boxed_slice();
        let (rx, mut tx) = tokio::io::split(AsyncFixedBuf::new(Box::leak(raw_buffer)));

        let join_handle = {
            let store = self.store.clone();
            let hash = stream.hash.clone();
            let expected_size = stream.expected_size;
            tokio::spawn(async move {
                let rx = Box::new(rx.take(expected_size as u64));
                store.update(&hash, expected_size, rx).await
            })
        };

        while let Some(write_request) = stream.next().await? {
            tx.write_all(&write_request.data)
                .await
                .or_else(|e| Err(Status::internal(format!("Error writing to store: {:?}", e))))?;
        }
        join_handle
            .await
            .or_else(|e| Err(Status::internal(format!("Error joining promise {:?}", e))))?
            .or_else(|e| Err(Status::internal(format!("Error joining promise {:?}", e))))?;
        Ok(Response::new(WriteResponse {
            committed_size: stream.bytes_received as i64,
        }))
    }
}

struct ResourceInfo<'a> {
    // TODO(allada) We do not support instance naming yet.
    _instance_name: &'a str,
    // TODO(allada) Currently we do not support stream resuming, this is
    // the field we would need.
    _uuid: &'a str,
    hash: &'a str,
    expected_size: usize,
}

impl<'a> ResourceInfo<'a> {
    fn new(resource_name: &'a str) -> Result<ResourceInfo<'a>, Status> {
        let mut parts = resource_name.splitn(6, '/');
        fn make_count_err() -> Status {
            Status::invalid_argument(format!(
                "Expected resource_name to be of pattern {}",
                "'{{instance_name}}/uploads/{{uuid}}/blobs/{{hash}}/{{size}}'"
            ))
        }
        let instance_name = &parts.next().ok_or_else(make_count_err)?;
        let uploads = &parts.next().ok_or_else(make_count_err)?;
        error_if!(
            uploads != &"uploads",
            Status::invalid_argument(format!(
                "Element 2 of resource_name should have been 'uploads'. Got: {:?}",
                uploads
            ))
        );
        let uuid = &parts.next().ok_or_else(make_count_err)?;
        let blobs = &parts.next().ok_or_else(make_count_err)?;
        error_if!(
            blobs != &"blobs",
            Status::invalid_argument(format!(
                "Element 4 of resource_name should have been 'blobs'. Got: {:?}",
                blobs
            ))
        );
        let hash = &parts.next().ok_or_else(make_count_err)?;
        let raw_digest_size = parts.next().ok_or_else(make_count_err)?;
        let expected_size = raw_digest_size
            .parse::<usize>()
            .or(Err(Status::invalid_argument(format!(
                "Digest size_bytes was not convertable to usize. Got: {:?}",
                raw_digest_size
            ))))?;
        Ok(ResourceInfo {
            _instance_name: instance_name,
            _uuid: uuid,
            hash,
            expected_size,
        })
    }
}

struct WriteRequestStreamWrapper {
    stream: Streaming<WriteRequest>,
    current_msg: WriteRequest,
    original_resource_name: String,
    hash: String,
    expected_size: usize,
    is_first: bool,
    bytes_received: usize,
}

impl WriteRequestStreamWrapper {
    async fn from(
        mut stream: Streaming<WriteRequest>,
    ) -> Result<WriteRequestStreamWrapper, Status> {
        let current_msg = stream
            .message()
            .await?
            .ok_or(make_input_err!("Expected WriteRequest struct in stream"))?;

        let original_resource_name = current_msg.resource_name.clone();
        let resource_info = ResourceInfo::new(&original_resource_name)?;
        let hash = resource_info.hash.to_string();
        let expected_size = resource_info.expected_size;
        Ok(WriteRequestStreamWrapper {
            stream,
            current_msg,
            original_resource_name,
            hash,
            expected_size,
            is_first: true,
            bytes_received: 0,
        })
    }

    async fn next<'a>(&'a mut self) -> Result<Option<&'a WriteRequest>, Status> {
        if self.is_first {
            self.is_first = false;
            self.bytes_received += self.current_msg.data.len();
            return Ok(Some(&self.current_msg));
        }
        if self.current_msg.finish_write {
            error_if!(
                self.bytes_received != self.expected_size,
                Status::invalid_argument(format!(
                    "Did not send enough data. Expected {}, but so far received {}",
                    self.expected_size, self.bytes_received
                ))
            );
            return Ok(None); // Previous message said it was the last msg.
        }
        error_if!(
            self.bytes_received > self.expected_size,
            Status::invalid_argument(format!(
                "Sent too much data. Expected {}, but so far received {}",
                self.expected_size, self.bytes_received
            ))
        );
        self.current_msg = self
            .stream
            .message()
            .await?
            .ok_or(make_input_err!("Expected WriteRequest struct in stream"))?;
        self.bytes_received += self.current_msg.data.len();

        error_if!(
            self.original_resource_name != self.current_msg.resource_name,
            Status::invalid_argument(format!(
                "Resource name missmatch, expected {:?} got {:?}",
                self.original_resource_name, self.current_msg.resource_name
            ))
        );

        Ok(Some(&self.current_msg))
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
        grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        // TODO(allada) We should do better logging here.
        println!("Write Req: {:?}", grpc_request);
        let resp = self.inner_write(grpc_request).await;
        println!("Write Resp: {:?}", resp);
        resp
    }

    async fn query_write_status(
        &self,
        _grpc_request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        println!("query_write_status {:?}", _grpc_request);
        Err(Status::unimplemented(""))
    }
}
