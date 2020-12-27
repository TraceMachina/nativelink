// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

#![feature(try_blocks)]

use std::convert::TryFrom;
use std::io::Cursor;
use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;
use tokio::io::Error;
use tonic::{Request, Response, Status};

use common;
use macros::{error_if, make_input_err};
use proto::build::bazel::remote::execution::v2::{
    batch_read_blobs_response, batch_update_blobs_response,
    content_addressable_storage_server::ContentAddressableStorage,
    content_addressable_storage_server::ContentAddressableStorageServer as Server,
    BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, FindMissingBlobsRequest, FindMissingBlobsResponse, GetTreeRequest,
    GetTreeResponse,
};
use store::Store;

#[derive(Debug)]
pub struct CasServer {
    store: Arc<dyn Store>,
}

impl CasServer {
    pub fn new(store: Arc<dyn Store>) -> Self {
        CasServer { store: store }
    }

    pub fn into_service(self) -> Server<CasServer> {
        Server::new(self)
    }
}

#[tonic::async_trait]
impl ContentAddressableStorage for CasServer {
    async fn find_missing_blobs(
        &self,
        request: Request<FindMissingBlobsRequest>,
    ) -> Result<Response<FindMissingBlobsResponse>, Status> {
        let request_data = request.into_inner();
        let mut response = FindMissingBlobsResponse {
            missing_blob_digests: vec![],
        };
        for digest in request_data.blob_digests.into_iter() {
            let result_status = self.store.has(&digest.hash, digest.hash.len()).await;
            if !result_status.unwrap_or(false) {
                // TODO(allada) We should log somewhere in the event result_status.is_err() (like bad hash).
                response.missing_blob_digests.push(digest.clone());
            }
        }
        Ok(Response::new(response))
    }

    async fn batch_update_blobs(
        &self,
        grpc_request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Status> {
        let batch_request = grpc_request.into_inner();
        let mut batch_response = BatchUpdateBlobsResponse {
            responses: Vec::with_capacity(batch_request.requests.len()),
        };
        for request in batch_request.requests {
            let orig_digest = request.digest.clone();
            let result_status: Result<(), Error> = try {
                let digest = request
                    .digest
                    .ok_or(make_input_err!("Digest not found in request"))?;
                let size_bytes = usize::try_from(digest.size_bytes).or(Err(make_input_err!(
                    "Digest size_bytes was not convertable to usize"
                )))?;
                error_if!(
                    size_bytes != request.data.len(),
                    make_input_err!(
                        "Digest for upload had mismatching sizes, digest said {} data  said {}",
                        size_bytes,
                        request.data.len()
                    )
                );
                self.store
                    .update(
                        &digest.hash,
                        size_bytes,
                        Box::new(Cursor::new(request.data)),
                    )
                    .await?;
            };
            let response = batch_update_blobs_response::Response {
                digest: orig_digest,
                status: Some(common::result_to_grpc_status(result_status)),
            };
            batch_response.responses.push(response);
        }
        Ok(Response::new(batch_response))
    }

    async fn batch_read_blobs(
        &self,
        grpc_request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Status> {
        let batch_read_request = grpc_request.into_inner();
        let mut batch_response = BatchReadBlobsResponse {
            responses: Vec::with_capacity(batch_read_request.digests.len()),
        };
        for digest in batch_read_request.digests {
            let size_bytes = usize::try_from(digest.size_bytes).or(Err(make_input_err!(
                "Digest size_bytes was not convertable to usize"
            )))?;
            // TODO(allada) There is a security risk here of someone taking all the memory on the instance.
            let mut store_data = Vec::with_capacity(size_bytes);
            let result_status: Result<(), Error> = try {
                self.store
                    .get(&digest.hash, size_bytes, &mut Cursor::new(&mut store_data))
                    .await?;
            };
            let response = batch_read_blobs_response::Response {
                digest: Some(digest.clone()),
                data: store_data,
                status: Some(common::result_to_grpc_status(result_status)),
            };
            batch_response.responses.push(response);
        }
        Ok(Response::new(batch_response))
    }

    type GetTreeStream =
        Pin<Box<dyn Stream<Item = Result<GetTreeResponse, Status>> + Send + Sync + 'static>>;
    async fn get_tree(
        &self,
        _request: Request<GetTreeRequest>,
    ) -> Result<Response<Self::GetTreeStream>, Status> {
        use stdext::function_name;
        let output = format!("{} not yet implemented", function_name!());
        println!("{}", output);
        Err(Status::unimplemented(output))
    }
}
