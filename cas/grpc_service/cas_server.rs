// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::pin::Pin;

use futures_core::Stream;
use tonic::{Request, Response, Status};

use proto::build::bazel::remote::execution::v2::{
    content_addressable_storage_server::ContentAddressableStorage,
    content_addressable_storage_server::ContentAddressableStorageServer as Server,
    BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, FindMissingBlobsRequest, FindMissingBlobsResponse, GetTreeRequest,
    GetTreeResponse,
};
use store::Store;

#[derive(Debug)]
pub struct CasServer {
    pub store: Box<dyn Store>,
}

impl CasServer {
    pub fn new(store: Box<dyn Store>) -> Self {
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
            if !self.store.has(&digest.hash, digest.hash.len()).await? {
                response.missing_blob_digests.push(digest);
            }
        }
        Ok(Response::new(response))
    }

    async fn batch_update_blobs(
        &self,
        _request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Status> {
        use stdext::function_name;
        let output = format!("{} not yet implemented", function_name!());
        println!("{}", output);
        Err(Status::unimplemented(output))
    }

    async fn batch_read_blobs(
        &self,
        _request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Status> {
        use stdext::function_name;
        let output = format!("{} not yet implemented", function_name!());
        println!("{}", output);
        Err(Status::unimplemented(output))
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
