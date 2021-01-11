// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::convert::TryFrom;
use std::convert::TryInto;
use std::io::Cursor;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use futures::{stream::Stream, FutureExt, StreamExt};
use tonic::{Request, Response, Status};

use proto::build::bazel::remote::execution::v2::{
    batch_read_blobs_response, batch_update_blobs_response,
    content_addressable_storage_server::ContentAddressableStorage,
    content_addressable_storage_server::ContentAddressableStorageServer as Server, BatchReadBlobsRequest,
    BatchReadBlobsResponse, BatchUpdateBlobsRequest, BatchUpdateBlobsResponse, FindMissingBlobsRequest,
    FindMissingBlobsResponse, GetTreeRequest, GetTreeResponse,
};
use proto::google::rpc::Status as GrpcStatus;

use common::{log, DigestInfo};
use error::{error_if, Error, ResultExt};
use store::Store;

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

    async fn inner_find_missing_blobs(
        &self,
        request: Request<FindMissingBlobsRequest>,
    ) -> Result<Response<FindMissingBlobsResponse>, Error> {
        let mut futures = futures::stream::FuturesOrdered::new();
        for digest in request.into_inner().blob_digests.into_iter() {
            let store_owned = self.store.clone();
            let digest: DigestInfo = digest.try_into()?;
            futures.push(tokio::spawn(async move {
                let store = Pin::new(store_owned.as_ref());
                store
                    .has(digest.clone())
                    .await
                    .map_or_else(|_| None, |success| if success { None } else { Some(digest) })
            }));
        }
        let mut responses = Vec::with_capacity(futures.len());
        while let Some(result) = futures.next().await {
            let val = result.err_tip(|| "Internal error joining future")?;
            if let Some(digest) = val {
                responses.push(digest.into());
            }
        }
        Ok(Response::new(FindMissingBlobsResponse {
            missing_blob_digests: responses,
        }))
    }

    async fn inner_batch_update_blobs(
        &self,
        grpc_request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Error> {
        let mut futures = futures::stream::FuturesOrdered::new();
        for request in grpc_request.into_inner().requests {
            let digest: DigestInfo = request.digest.err_tip(|| "Digest not found in request")?.try_into()?;
            let digest_copy = digest.clone();
            let store_owned = self.store.clone();
            let request_data = request.data;
            futures.push(tokio::spawn(
                async move {
                    let size_bytes = usize::try_from(digest_copy.size_bytes)
                        .err_tip(|| "Digest size_bytes was not convertable to usize")?;
                    error_if!(
                        size_bytes != request_data.len(),
                        "Digest for upload had mismatching sizes, digest said {} data  said {}",
                        size_bytes,
                        request_data.len()
                    );
                    let cursor = Box::new(Cursor::new(request_data));
                    let store = Pin::new(store_owned.as_ref());
                    store
                        .update(digest_copy, cursor)
                        .await
                        .err_tip(|| "Error writing to store")
                }
                .map(|result| batch_update_blobs_response::Response {
                    digest: Some(digest.into()),
                    status: Some(result.map_or_else(|e| e.into(), |_| GrpcStatus::default())),
                }),
            ));
        }
        let mut responses = Vec::with_capacity(futures.len());
        while let Some(result) = futures.next().await {
            responses.push(result.err_tip(|| "Internal error joining future")?);
        }
        Ok(Response::new(BatchUpdateBlobsResponse { responses: responses }))
    }

    async fn inner_batch_read_blobs(
        &self,
        grpc_request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Error> {
        let mut futures = futures::stream::FuturesOrdered::new();

        for digest in grpc_request.into_inner().digests {
            let digest: DigestInfo = digest.try_into()?;
            let digest_copy = digest.clone();
            let store_owned = self.store.clone();

            futures.push(tokio::spawn(
                async move {
                    let size_bytes = usize::try_from(digest_copy.size_bytes)
                        .err_tip(|| "Digest size_bytes was not convertable to usize")?;
                    // TODO(allada) There is a security risk here of someone taking all the memory on the instance.
                    let mut store_data = Vec::with_capacity(size_bytes);
                    let store = Pin::new(store_owned.as_ref());
                    store
                        .get(digest_copy, &mut Cursor::new(&mut store_data))
                        .await
                        .err_tip(|| "Error reading from store")?;
                    Ok(store_data)
                }
                .map(|result: Result<Vec<u8>, Error>| {
                    let (status, data) = result.map_or_else(|e| (e.into(), vec![]), |v| (GrpcStatus::default(), v));
                    batch_read_blobs_response::Response {
                        status: Some(status),
                        digest: Some(digest.into()),
                        data: data,
                    }
                }),
            ));
        }

        let mut responses = Vec::with_capacity(futures.len());
        while let Some(result) = futures.next().await {
            responses.push(result.err_tip(|| "Internal error joining future")?);
        }
        Ok(Response::new(BatchReadBlobsResponse { responses: responses }))
    }
}

#[tonic::async_trait]
impl ContentAddressableStorage for CasServer {
    async fn find_missing_blobs(
        &self,
        grpc_request: Request<FindMissingBlobsRequest>,
    ) -> Result<Response<FindMissingBlobsResponse>, Status> {
        log::info!("\x1b[0;31mfind_missing_blobs Req\x1b[0m: {:?}", grpc_request.get_ref());
        let now = Instant::now();
        let resp = self
            .inner_find_missing_blobs(grpc_request)
            .await
            .err_tip(|| format!("Failed on find_missing_blobs() command"))
            .map_err(|e| e.into());
        let d = now.elapsed().as_secs_f32();
        log::info!("\x1b[0;31mfind_missing_blobs Resp\x1b[0m: {} {:?}", d, resp);
        resp
    }

    async fn batch_update_blobs(
        &self,
        grpc_request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Status> {
        log::info!("\x1b[0;31mbatch_update_blobs Req\x1b[0m: {:?}", grpc_request.get_ref());
        let now = Instant::now();
        let resp = self
            .inner_batch_update_blobs(grpc_request)
            .await
            .err_tip(|| format!("Failed on batch_update_blobs() command"))
            .map_err(|e| e.into());
        let d = now.elapsed().as_secs_f32();
        log::info!("\x1b[0;31mbatch_update_blobs Resp\x1b[0m: {} {:?}", d, resp);
        resp
    }

    async fn batch_read_blobs(
        &self,
        grpc_request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Status> {
        log::info!("\x1b[0;31mbatch_read_blobs Req\x1b[0m: {:?}", grpc_request.get_ref());
        let now = Instant::now();
        let resp = self
            .inner_batch_read_blobs(grpc_request)
            .await
            .err_tip(|| format!("Failed on batch_read_blobs() command"))
            .map_err(|e| e.into());
        let d = now.elapsed().as_secs_f32();
        log::info!("\x1b[0;31mbatch_read_blobs Resp\x1b[0m: {} {:?}", d, resp);
        resp
    }

    type GetTreeStream = Pin<Box<dyn Stream<Item = Result<GetTreeResponse, Status>> + Send + Sync + 'static>>;
    async fn get_tree(&self, _request: Request<GetTreeRequest>) -> Result<Response<Self::GetTreeStream>, Status> {
        use stdext::function_name;
        let output = format!("{} not yet implemented", function_name!());
        log::info!("\x1b[0;31mget_tree\x1b[0m: {:?}", output);
        Err(Status::unimplemented(output))
    }
}
