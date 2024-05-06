// Copyright 2023 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use futures::stream::{FuturesUnordered, Stream};
use futures::TryStreamExt;
use nativelink_config::cas_server::{CasStoreConfig, InstanceName};
use nativelink_error::{error_if, make_err, make_input_err, Code, Error, ResultExt};
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_server::{
    ContentAddressableStorage, ContentAddressableStorageServer as Server,
};
use nativelink_proto::build::bazel::remote::execution::v2::{
    batch_read_blobs_response, batch_update_blobs_response, compressor, BatchReadBlobsRequest,
    BatchReadBlobsResponse, BatchUpdateBlobsRequest, BatchUpdateBlobsResponse,
    FindMissingBlobsRequest, FindMissingBlobsResponse, GetTreeRequest, GetTreeResponse,
};
use nativelink_proto::google::rpc::Status as GrpcStatus;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::make_ctx_for_hash_func;
use nativelink_util::store_trait::Store;
use tonic::{Request, Response, Status};
use tracing::{error_span, event, instrument, Level};

pub struct CasServer {
    stores: HashMap<String, Arc<dyn Store>>,
}

type GetTreeStream = Pin<Box<dyn Stream<Item = Result<GetTreeResponse, Status>> + Send + 'static>>;

impl CasServer {
    pub fn new(
        config: &HashMap<InstanceName, CasStoreConfig>,
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut stores = HashMap::with_capacity(config.len());
        for (instance_name, cas_cfg) in config {
            let store = store_manager.get_store(&cas_cfg.cas_store).ok_or_else(|| {
                make_input_err!("'cas_store': '{}' does not exist", cas_cfg.cas_store)
            })?;
            stores.insert(instance_name.to_string(), store);
        }
        Ok(CasServer { stores })
    }

    pub fn into_service(self) -> Server<CasServer> {
        Server::new(self)
    }

    async fn inner_find_missing_blobs(
        &self,
        request: FindMissingBlobsRequest,
    ) -> Result<Response<FindMissingBlobsResponse>, Error> {
        let instance_name = &request.instance_name;
        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();

        let mut requested_blobs = Vec::with_capacity(request.blob_digests.len());
        for digest in request.blob_digests.iter() {
            requested_blobs.push(DigestInfo::try_from(digest.clone())?);
        }
        let sizes = Pin::new(store.as_ref())
            .has_many(&requested_blobs)
            .await
            .err_tip(|| "In find_missing_blobs")?;
        let missing_blob_digests = sizes
            .into_iter()
            .zip(request.blob_digests)
            .filter_map(|(maybe_size, digest)| maybe_size.map_or_else(|| Some(digest), |_| None))
            .collect();

        Ok(Response::new(FindMissingBlobsResponse {
            missing_blob_digests,
        }))
    }

    async fn inner_batch_update_blobs(
        &self,
        request: BatchUpdateBlobsRequest,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Error> {
        let instance_name = &request.instance_name;

        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();

        // If we are a GrpcStore we shortcut here, as this is a special store.
        // Note: We don't know the digests here, so we try perform a very shallow
        // check to see if it's a grpc store.
        let any_store = store.inner_store(None).as_any();
        if let Some(grpc_store) = any_store.downcast_ref::<GrpcStore>() {
            return grpc_store.batch_update_blobs(Request::new(request)).await;
        }

        let store_pin = Pin::new(store.as_ref());
        let update_futures: FuturesUnordered<_> = request
            .requests
            .into_iter()
            .map(|request| async move {
                let digest = request
                    .digest
                    .clone()
                    .err_tip(|| "Digest not found in request")?;
                let request_data = request.data;
                let digest_info = DigestInfo::try_from(digest.clone())?;
                let size_bytes = usize::try_from(digest_info.size_bytes)
                    .err_tip(|| "Digest size_bytes was not convertible to usize")?;
                error_if!(
                    size_bytes != request_data.len(),
                    "Digest for upload had mismatching sizes, digest said {} data  said {}",
                    size_bytes,
                    request_data.len()
                );
                let result = store_pin
                    .update_oneshot(digest_info, request_data)
                    .await
                    .err_tip(|| "Error writing to store");
                Ok::<_, Error>(batch_update_blobs_response::Response {
                    digest: Some(digest),
                    status: Some(result.map_or_else(|e| e.into(), |_| GrpcStatus::default())),
                })
            })
            .collect();
        let responses = update_futures
            .try_collect::<Vec<batch_update_blobs_response::Response>>()
            .await?;

        Ok(Response::new(BatchUpdateBlobsResponse { responses }))
    }

    async fn inner_batch_read_blobs(
        &self,
        request: BatchReadBlobsRequest,
    ) -> Result<Response<BatchReadBlobsResponse>, Error> {
        let instance_name = &request.instance_name;

        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();

        // If we are a GrpcStore we shortcut here, as this is a special store.
        // Note: We don't know the digests here, so we try perform a very shallow
        // check to see if it's a grpc store.
        let any_store = store.inner_store(None).as_any();
        if let Some(grpc_store) = any_store.downcast_ref::<GrpcStore>() {
            return grpc_store.batch_read_blobs(Request::new(request)).await;
        }

        let store_pin = Pin::new(store.as_ref());
        let read_futures: FuturesUnordered<_> = request
            .digests
            .into_iter()
            .map(|digest| async move {
                let digest_copy = DigestInfo::try_from(digest.clone())?;
                // TODO(allada) There is a security risk here of someone taking all the memory on the instance.
                let result = store_pin
                    .get_part_unchunked(digest_copy, 0, None)
                    .await
                    .err_tip(|| "Error reading from store");
                let (status, data) = result.map_or_else(
                    |mut e| {
                        if e.code == Code::NotFound {
                            // Trim the error code. Not Found is quite common and we don't want to send a large
                            // error (debug) message for something that is common. We resize to just the last
                            // message as it will be the most relevant.
                            e.messages.resize_with(1, || "".to_string());
                        }
                        (e.into(), Bytes::new())
                    },
                    |v| (GrpcStatus::default(), v),
                );
                Ok::<_, Error>(batch_read_blobs_response::Response {
                    status: Some(status),
                    digest: Some(digest),
                    compressor: compressor::Value::Identity.into(),
                    data,
                })
            })
            .collect();
        let responses = read_futures
            .try_collect::<Vec<batch_read_blobs_response::Response>>()
            .await?;

        Ok(Response::new(BatchReadBlobsResponse { responses }))
    }

    async fn inner_get_tree(
        &self,
        request: GetTreeRequest,
    ) -> Result<Response<GetTreeStream>, Error> {
        let instance_name = &request.instance_name;

        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();

        // If we are a GrpcStore we shortcut here, as this is a special store.
        // Note: We don't know the digests here, so we try perform a very shallow
        // check to see if it's a grpc store.
        let any_store = store.inner_store(None).as_any();
        if let Some(grpc_store) = any_store.downcast_ref::<GrpcStore>() {
            let stream = grpc_store
                .get_tree(Request::new(request))
                .await?
                .into_inner();
            return Ok(Response::new(Box::pin(stream)));
        }
        Err(make_err!(
            Code::Unimplemented,
            "get_tree is not implemented"
        ))
    }
}

#[tonic::async_trait]
impl ContentAddressableStorage for CasServer {
    type GetTreeStream = GetTreeStream;

    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn find_missing_blobs(
        &self,
        grpc_request: Request<FindMissingBlobsRequest>,
    ) -> Result<Response<FindMissingBlobsResponse>, Status> {
        let request = grpc_request.into_inner();
        make_ctx_for_hash_func(request.digest_function)
            .err_tip(|| "In CasServer::find_missing_blobs")?
            .wrap_async(
                error_span!("cas_server_find_missing_blobs"),
                self.inner_find_missing_blobs(request),
            )
            .await
            .err_tip(|| "Failed on find_missing_blobs() command")
            .map_err(|e| e.into())
    }

    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn batch_update_blobs(
        &self,
        grpc_request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Status> {
        let request = grpc_request.into_inner();
        make_ctx_for_hash_func(request.digest_function)
            .err_tip(|| "In CasServer::batch_update_blobs")?
            .wrap_async(
                error_span!("cas_server_batch_update_blobs"),
                self.inner_batch_update_blobs(request),
            )
            .await
            .err_tip(|| "Failed on batch_update_blobs() command")
            .map_err(|e| e.into())
    }

    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn batch_read_blobs(
        &self,
        grpc_request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Status> {
        let request = grpc_request.into_inner();
        make_ctx_for_hash_func(request.digest_function)
            .err_tip(|| "In CasServer::batch_read_blobs")?
            .wrap_async(
                error_span!("cas_server_batch_read_blobs"),
                self.inner_batch_read_blobs(request),
            )
            .await
            .err_tip(|| "Failed on batch_read_blobs() command")
            .map_err(|e| e.into())
    }

    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err,
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn get_tree(
        &self,
        grpc_request: Request<GetTreeRequest>,
    ) -> Result<Response<Self::GetTreeStream>, Status> {
        let request = grpc_request.into_inner();
        let resp = make_ctx_for_hash_func(request.digest_function)
            .err_tip(|| "In CasServer::get_tree")?
            .wrap_async(
                error_span!("cas_server_get_tree"),
                self.inner_get_tree(request),
            )
            .await
            .err_tip(|| "Failed on get_tree() command")
            .map_err(|e| e.into());
        if resp.is_ok() {
            event!(Level::DEBUG, return = "Ok(<stream>)");
        }
        resp
    }
}
