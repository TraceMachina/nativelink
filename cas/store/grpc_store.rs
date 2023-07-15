// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use futures::{stream::unfold, Stream};
use prost::Message;
use tonic::{transport, IntoRequest, Request, Response, Streaming};
use uuid::Uuid;

use ac_utils::ESTIMATED_DIGEST_SIZE;
use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::{log, DigestInfo};
use error::{error_if, make_input_err, Error, ResultExt};
use parking_lot::Mutex;
use proto::build::bazel::remote::execution::v2::{
    action_cache_client::ActionCacheClient, content_addressable_storage_client::ContentAddressableStorageClient,
    ActionResult, BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest, BatchUpdateBlobsResponse,
    FindMissingBlobsRequest, FindMissingBlobsResponse, GetActionResultRequest, GetTreeRequest, GetTreeResponse,
    UpdateActionResultRequest,
};
use proto::google::bytestream::{
    byte_stream_client::ByteStreamClient, QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse,
    WriteRequest, WriteResponse,
};
use traits::{StoreTrait, UploadSizeInfo};
use write_request_stream_wrapper::WriteRequestStreamWrapper;

// This store is usually a pass-through store, but can also be used as a CAS store. Using it as an
// AC store has one major side-effect... The has() function may not give the proper size of the
// underlying data. This might cause issues if embedded in certain stores.
pub struct GrpcStore {
    instance_name: String,
    cas_client: ContentAddressableStorageClient<transport::Channel>,
    bytestream_client: ByteStreamClient<transport::Channel>,
    ac_client: ActionCacheClient<transport::Channel>,
    store_type: config::stores::StoreType,
}

impl GrpcStore {
    pub async fn new(config: &config::stores::GrpcStore) -> Result<Self, Error> {
        error_if!(config.endpoints.is_empty(), "Expected at least 1 endpoint in GrpcStore");
        let mut endpoints = Vec::with_capacity(config.endpoints.len());
        for endpoint in &config.endpoints {
            // TODO(allada) This should be moved to be done in utils/serde_utils.rs like the others.
            // We currently don't have a way to handle those helpers with vectors.
            let endpoint = shellexpand::env(&endpoint)
                .map_err(|e| make_input_err!("{}", e))
                .err_tip(|| "Could expand endpoint in GrpcStore")?
                .to_string();

            endpoints.push(
                transport::Endpoint::new(endpoint.clone())
                    .err_tip(|| format!("Could not connect to {} in GrpcStore", endpoint))?,
            );
        }

        let conn = transport::Channel::balance_list(endpoints.into_iter());
        Ok(GrpcStore {
            instance_name: config.instance_name.clone(),
            cas_client: ContentAddressableStorageClient::new(conn.clone()),
            bytestream_client: ByteStreamClient::new(conn.clone()),
            ac_client: ActionCacheClient::new(conn),
            store_type: config.store_type,
        })
    }

    pub async fn find_missing_blobs(
        &self,
        grpc_request: Request<FindMissingBlobsRequest>,
    ) -> Result<Response<FindMissingBlobsResponse>, Error> {
        error_if!(
            matches!(self.store_type, config::stores::StoreType::ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();
        request.instance_name = self.instance_name.clone();
        let mut client = self.cas_client.clone();
        client
            .find_missing_blobs(Request::new(request))
            .await
            .err_tip(|| "in GrpcStore::find_missing_blobs")
    }

    pub async fn batch_update_blobs(
        &self,
        grpc_request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Error> {
        error_if!(
            matches!(self.store_type, config::stores::StoreType::ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();
        request.instance_name = self.instance_name.clone();
        let mut client = self.cas_client.clone();
        client
            .batch_update_blobs(Request::new(request))
            .await
            .err_tip(|| "in GrpcStore::batch_update_blobs")
    }

    pub async fn batch_read_blobs(
        &self,
        grpc_request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Error> {
        error_if!(
            matches!(self.store_type, config::stores::StoreType::ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();
        request.instance_name = self.instance_name.clone();
        let mut client = self.cas_client.clone();
        client
            .batch_read_blobs(Request::new(request))
            .await
            .err_tip(|| "in GrpcStore::batch_read_blobs")
    }

    pub async fn get_tree(
        &self,
        grpc_request: Request<GetTreeRequest>,
    ) -> Result<Response<Streaming<GetTreeResponse>>, Error> {
        error_if!(
            matches!(self.store_type, config::stores::StoreType::ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();
        request.instance_name = self.instance_name.clone();
        let mut client = self.cas_client.clone();
        client
            .get_tree(Request::new(request))
            .await
            .err_tip(|| "in GrpcStore::get_tree")
    }

    pub async fn read(
        &self,
        grpc_request: impl IntoRequest<ReadRequest>,
    ) -> Result<Response<Streaming<ReadResponse>>, Error> {
        error_if!(
            matches!(self.store_type, config::stores::StoreType::ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_request().into_inner();

        // `resource_name` pattern is: "{instance_name}/blobs/{hash}/{size}".
        let first_slash_pos = request
            .resource_name
            .find('/')
            .err_tip(|| "Resource name expected to follow pattern {instance_name}/blobs/{hash}/{size}")?;
        request.resource_name = format!(
            "{}/{}",
            self.instance_name,
            request.resource_name.get((first_slash_pos + 1)..).unwrap()
        );

        let mut client = self.bytestream_client.clone();
        client
            .read(Request::new(request))
            .await
            .err_tip(|| "in GrpcStore::read")
    }

    pub async fn write<T, E>(&self, stream: WriteRequestStreamWrapper<T, E>) -> Result<Response<WriteResponse>, Error>
    where
        T: Stream<Item = Result<WriteRequest, E>> + Unpin + Send + 'static,
        E: Into<Error> + 'static,
    {
        error_if!(
            matches!(self.store_type, config::stores::StoreType::ac),
            "CAS operation on AC store"
        );

        let mut client = self.bytestream_client.clone();

        let error = Arc::new(Mutex::new(None));
        struct LocalState {
            instance_name: String,
            error: Arc<Mutex<Option<Error>>>,
        }

        let local_state = LocalState {
            instance_name: self.instance_name.clone(),
            error: error.clone(),
        };

        let stream = unfold((stream, local_state), move |(mut stream, local_state)| async {
            let maybe_message = stream.next().await;
            if let Ok(maybe_message) = maybe_message {
                if let Some(mut message) = maybe_message {
                    // `resource_name` pattern is: "{instance_name}/uploads/{uuid}/blobs/{hash}/{size}".
                    let first_slash_pos = match message.resource_name.find('/') {
                        Some(pos) => pos,
                        None => {
                            log::error!("{}", "Resource name should follow pattern {instance_name}/uploads/{uuid}/blobs/{hash}/{size}");
                            return None;
                        }
                    };
                    message.resource_name = format!(
                        "{}/{}",
                        &local_state.instance_name,
                        message.resource_name.get((first_slash_pos + 1)..).unwrap()
                    );
                    return Some((message, (stream, local_state)));
                }
                return None;
            }
            // TODO(allada) I'm sure there's a way to do this without a mutex, but rust can be super
            // picky with borrowing through a stream await.
            *local_state.error.lock() = Some(maybe_message.unwrap_err());
            None
        });

        let result = client.write(stream).await.err_tip(|| "in GrpcStore::write")?;
        if let Some(err) = error.lock().take() {
            return Err(err);
        }
        Ok(result)
    }

    pub async fn query_write_status(
        &self,
        grpc_request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Error> {
        error_if!(
            matches!(self.store_type, config::stores::StoreType::ac),
            "CAS operation on AC store"
        );

        let mut request = grpc_request.into_inner();

        // `resource_name` pattern is: "{instance_name}/uploads/{uuid}/blobs/{hash}/{size}".
        let first_slash_pos = request.resource_name.find('/').err_tip(|| {
            "Resource name expected to follow pattern {instance_name}/uploads/{uuid}/blobs/{hash}/{size}"
        })?;
        request.resource_name = format!(
            "{}/{}",
            self.instance_name,
            request.resource_name.get((first_slash_pos + 1)..).unwrap()
        );

        let mut client = self.bytestream_client.clone();
        client
            .query_write_status(Request::new(request))
            .await
            .err_tip(|| "in GrpcStore::query_write_status")
    }

    pub async fn get_action_result(
        &self,
        grpc_request: Request<GetActionResultRequest>,
    ) -> Result<Response<ActionResult>, Error> {
        let mut request = grpc_request.into_inner();
        request.instance_name = self.instance_name.clone();
        let mut client = self.ac_client.clone();
        client
            .get_action_result(Request::new(request))
            .await
            .err_tip(|| "in GrpcStore::get_action_result")
    }

    pub async fn update_action_result(
        &self,
        grpc_request: Request<UpdateActionResultRequest>,
    ) -> Result<Response<ActionResult>, Error> {
        let mut request = grpc_request.into_inner();
        request.instance_name = self.instance_name.clone();
        let mut client = self.ac_client.clone();
        client
            .update_action_result(Request::new(request))
            .await
            .err_tip(|| "in GrpcStore::update_action_result")
    }

    async fn get_action_result_from_digest(&self, digest: DigestInfo) -> Result<Response<ActionResult>, Error> {
        let action_result_request = GetActionResultRequest {
            instance_name: self.instance_name.clone(),
            action_digest: Some(digest.into()),
            inline_stdout: false,
            inline_stderr: false,
            inline_output_files: Vec::new(),
        };
        self.get_action_result(Request::new(action_result_request)).await
    }

    async fn get_action_result_as_part(
        &self,
        digest: DigestInfo,
        mut writer: DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let action_result = self
            .get_action_result_from_digest(digest)
            .await
            .map(|response| response.into_inner())
            .ok()
            .err_tip(|| "Action result not found")?;
        // TODO: Would be create to avoid all the encoding and decoding in this
        //       file, however there's no way to currently get raw bytes from a
        //       generated prost request unfortunately.
        let mut value = BytesMut::new();
        action_result
            .encode(&mut value)
            .err_tip(|| "Could not encode upstream action result")?;

        let default_len = value.len() - offset;
        let length = length.unwrap_or(default_len).min(default_len);
        if length > 0 {
            writer
                .send(value.freeze().slice(offset..(offset + length)))
                .await
                .err_tip(|| "Failed to write data in grpc store")?;
        }
        writer
            .send_eof()
            .await
            .err_tip(|| "Failed to write EOF in grpc store get_action_result_as_part")?;
        Ok(())
    }

    async fn update_action_result_from_bytes(
        &self,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
    ) -> Result<(), Error> {
        let action_result = ActionResult::decode(reader.collect_all_with_size_hint(ESTIMATED_DIGEST_SIZE).await?)
            .err_tip(|| "Failed to decode ActionResult in update_action_result_from_bytes")?;
        let update_action_request = UpdateActionResultRequest {
            instance_name: self.instance_name.clone(),
            action_digest: Some(digest.into()),
            action_result: Some(action_result),
            results_cache_policy: None,
        };
        self.update_action_result(Request::new(update_action_request))
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl StoreTrait for GrpcStore {
    // NOTE: This function can only be safely used on CAS stores. AC stores may return a size that
    // is incorrect.
    async fn has(self: Pin<&Self>, digest: DigestInfo) -> Result<Option<usize>, Error> {
        if matches!(self.store_type, config::stores::StoreType::ac) {
            // The length of an AC is incorrect, so we don't figure out the
            // length, instead the biggest possible result is returned in the
            // hope that we detect incorrect usage.
            return self.get_action_result_from_digest(digest).await.map(|_| Some(usize::MAX));
        }

        let digest_size =
            usize::try_from(digest.size_bytes).err_tip(|| "GrpcStore digest size cannot be converted to usize")?;
        let missing_blobs_response = self
            .find_missing_blobs(Request::new(FindMissingBlobsRequest {
                instance_name: self.instance_name.clone(),
                blob_digests: vec![digest.into()],
            }))
            .await?
            .into_inner();
        if missing_blobs_response.missing_blob_digests.is_empty() {
            return Ok(Some(digest_size));
        }
        return Ok(None);
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        if matches!(self.store_type, config::stores::StoreType::ac) {
            return self.update_action_result_from_bytes(digest, reader).await;
        }

        let mut buf = Uuid::encode_buffer();
        let resource_name = format!(
            "{}/uploads/{}/blobs/{}/{}",
            &self.instance_name,
            Uuid::new_v4().hyphenated().encode_lower(&mut buf),
            digest.str(),
            digest.size_bytes,
        );

        struct LocalState {
            resource_name: String,
            reader: DropCloserReadHalf,
            did_error: bool,
            bytes_received: i64,
        }
        let local_state = LocalState {
            resource_name,
            reader,
            did_error: false,
            bytes_received: 0,
        };

        let stream = Box::pin(unfold(local_state, |mut local_state| async move {
            if local_state.did_error {
                log::error!("GrpcStore::update() polled stream after error was returned.");
                return None;
            }
            let data = match local_state.reader.recv().await.err_tip(|| "In GrpcStore::update()") {
                Ok(data) => data,
                Err(err) => {
                    local_state.did_error = true;
                    return Some((Err(err), local_state));
                }
            };

            let write_offset = local_state.bytes_received;
            local_state.bytes_received += data.len() as i64;

            Some((
                Ok(WriteRequest {
                    resource_name: local_state.resource_name.clone(),
                    write_offset,
                    finish_write: data.is_empty(), // EOF is when no data was polled.
                    data,
                }),
                local_state,
            ))
        }));

        self.write(
            WriteRequestStreamWrapper::from(stream)
                .await
                .err_tip(|| "in GrpcStore::update()")?,
        )
        .await
        .err_tip(|| "in GrpcStore::update()")?;

        Ok(())
    }

    async fn get_part(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut writer: DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        if matches!(self.store_type, config::stores::StoreType::ac) {
            return self.get_action_result_as_part(digest, writer, offset, length).await;
        }

        let resource_name = format!("{}/blobs/{}/{}", &self.instance_name, digest.str(), digest.size_bytes,);

        let mut stream = self
            .read(Request::new(ReadRequest {
                resource_name,
                read_offset: offset as i64,
                read_limit: length.unwrap_or(0) as i64,
            }))
            .await
            .err_tip(|| "in GrpcStore::get_part()")?
            .into_inner();

        loop {
            let maybe_message = stream
                .message()
                .await
                .err_tip(|| "While fetching message in GrpcStore::get_part()")?;
            let message = if let Some(message) = maybe_message {
                message
            } else {
                writer
                    .send_eof()
                    .await
                    .err_tip(|| "Could not send eof in GrpcStore::get_part()")?;
                break; // EOF.
            };
            if message.data.is_empty() {
                writer
                    .send_eof()
                    .await
                    .err_tip(|| "Could not send eof in GrpcStore::get_part()")?;
                break; // EOF.
            }
            writer
                .send(message.data)
                .await
                .err_tip(|| "While sending in GrpcStore::get_part()")?;
        }

        Ok(())
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
