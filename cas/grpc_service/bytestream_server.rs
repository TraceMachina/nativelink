// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use async_fixed_buffer::AsyncFixedBuf;
use futures::Stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tonic::{Request, Response, Status, Streaming};

use proto::google::bytestream::{
    byte_stream_server::ByteStream, byte_stream_server::ByteStreamServer as Server,
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};

use common::{log, DigestInfo};
use error::{error_if, Error, ResultExt};
use store::Store;

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
    ) -> Result<Response<WriteResponse>, Error> {
        let mut stream = WriteRequestStreamWrapper::from(grpc_request.into_inner())
            .await
            .err_tip(|| "Could not unwrap first stream message")?;

        let raw_buffer = vec![0u8; self.max_stream_buffer_size].into_boxed_slice();
        let (rx, mut tx) = tokio::io::split(AsyncFixedBuf::new(raw_buffer));

        let join_handle = {
            let store = self.store.clone();
            let hash = stream.hash.clone();
            let expected_size = stream.expected_size;
            tokio::spawn(async move {
                let rx = Box::new(rx.take(expected_size as u64));
                store
                    .update(&DigestInfo::try_new(&hash, expected_size)?, rx)
                    .await
            })
        };

        while let Some(write_request) = stream.next().await.err_tip(|| "Stream closed early")? {
            tx.write_all(&write_request.data)
                .await
                .err_tip(|| "Error writing to store stream")?;
        }
        join_handle
            .await
            .err_tip(|| "Error joining promise")?
            .err_tip(|| "Error updating inner store")?;
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
    fn new(resource_name: &'a str) -> Result<ResourceInfo<'a>, Error> {
        let mut parts = resource_name.splitn(6, '/');
        const ERROR_MSG: &str = concat!(
            "Expected resource_name to be of pattern ",
            "'{instance_name}/uploads/{uuid}/blobs/{hash}/{size}'"
        );
        let instance_name = &parts.next().err_tip(|| ERROR_MSG)?;
        let uploads = &parts.next().err_tip(|| ERROR_MSG)?;
        error_if!(
            uploads != &"uploads",
            "Element 2 of resource_name should have been 'uploads'. Got: {}",
            uploads
        );
        let uuid = &parts.next().err_tip(|| ERROR_MSG)?;
        let blobs = &parts.next().err_tip(|| ERROR_MSG)?;
        error_if!(
            blobs != &"blobs",
            "Element 4 of resource_name should have been 'blobs'. Got: {}",
            blobs
        );
        let hash = &parts.next().err_tip(|| ERROR_MSG)?;
        let raw_digest_size = parts.next().err_tip(|| ERROR_MSG)?;
        let expected_size = raw_digest_size.parse::<usize>().err_tip(|| {
            format!(
                "Digest size_bytes was not convertable to usize. Got: {}",
                raw_digest_size
            )
        })?;
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
    hash: String,
    expected_size: usize,
    is_first: bool,
    bytes_received: usize,
}

impl WriteRequestStreamWrapper {
    async fn from(mut stream: Streaming<WriteRequest>) -> Result<WriteRequestStreamWrapper, Error> {
        let current_msg = stream
            .message()
            .await
            .err_tip(|| "Error receiving first message in stream")?
            .err_tip(|| "Expected WriteRequest struct in stream")?;

        let resource_info = ResourceInfo::new(&current_msg.resource_name)
            .err_tip(|| "Could not extract resource info from first message of stream")?;
        let hash = resource_info.hash.to_string();
        let expected_size = resource_info.expected_size;
        Ok(WriteRequestStreamWrapper {
            stream,
            current_msg,
            hash,
            expected_size,
            is_first: true,
            bytes_received: 0,
        })
    }

    async fn next<'a>(&'a mut self) -> Result<Option<&'a WriteRequest>, Error> {
        if self.is_first {
            self.is_first = false;
            self.bytes_received += self.current_msg.data.len();
            return Ok(Some(&self.current_msg));
        }
        if self.current_msg.finish_write {
            error_if!(
                self.bytes_received != self.expected_size,
                "Did not send enough data. Expected {}, but so far received {}",
                self.expected_size,
                self.bytes_received
            );
            return Ok(None); // Previous message said it was the last msg.
        }
        error_if!(
            self.bytes_received > self.expected_size,
            "Sent too much data. Expected {}, but so far received {}",
            self.expected_size,
            self.bytes_received
        );
        self.current_msg = self
            .stream
            .message()
            .await
            .err_tip(|| format!("Stream error at byte {}", self.bytes_received))?
            .err_tip(|| "Expected WriteRequest struct in stream")?;
        self.bytes_received += self.current_msg.data.len();

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
        log::info!("\x1b[0;31mread\x1b[0m {:?}", _grpc_request.get_ref());
        Err(Status::unimplemented(""))
    }

    async fn write(
        &self,
        grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        log::info!("\x1b[0;31mWrite Req\x1b[0m: {:?}", grpc_request.get_ref());
        let now = Instant::now();
        let resp = self
            .inner_write(grpc_request)
            .await
            .err_tip(|| format!("Failed on write() command"))
            .map_err(|e| e.into());
        let d = now.elapsed().as_secs_f32();
        log::info!("\x1b[0;31mWrite Resp\x1b[0m: {} {:?}", d, resp);
        resp
    }

    async fn query_write_status(
        &self,
        _grpc_request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        log::info!("query_write_status {:?}", _grpc_request.get_ref());
        Err(Status::unimplemented(""))
    }
}
