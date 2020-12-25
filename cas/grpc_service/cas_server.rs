// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::pin::Pin;

use futures_core::Stream;
use tonic::{Request, Response, Status};

use proto::build::bazel::remote::execution::v2::{
    content_addressable_storage_server::ContentAddressableStorage,
    content_addressable_storage_server::ContentAddressableStorageServer,
    BatchReadBlobsRequest,
    BatchReadBlobsResponse, BatchUpdateBlobsRequest, BatchUpdateBlobsResponse,
    FindMissingBlobsRequest, FindMissingBlobsResponse, GetTreeRequest, GetTreeResponse,
};

#[derive(Debug, Default)]
pub struct CasServer {}

impl CasServer {
    pub fn into_service(self) -> ContentAddressableStorageServer<CasServer> {
        ContentAddressableStorageServer::new(self)
    }
}

#[tonic::async_trait]
impl ContentAddressableStorage for CasServer {
    async fn find_missing_blobs(
        &self,
        _request: Request<FindMissingBlobsRequest>,
    ) -> Result<Response<FindMissingBlobsResponse>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn batch_update_blobs(
        &self,
        _request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn batch_read_blobs(
        &self,
        _request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    type GetTreeStream =
        Pin<Box<dyn Stream<Item = Result<GetTreeResponse, Status>> + Send + Sync + 'static>>;
    async fn get_tree(
        &self,
        _request: Request<GetTreeRequest>,
    ) -> Result<Response<Self::GetTreeStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
}
