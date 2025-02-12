use std::sync::Arc;
use std::time::Duration;

use futures::{StreamExt, TryStreamExt};
use googleapis_tonic_google_storage_v2::google::storage::v2::{
    bidi_write_object_request, write_object_request, BidiWriteObjectRequest, ChecksummedData,
    Object, ReadObjectRequest, StartResumableWriteRequest, WriteObjectRequest, WriteObjectSpec,
};
use nativelink_config::stores::GcsSpec;
use nativelink_error::{make_err, Code, Error};
use nativelink_util::buf_channel::DropCloserReadHalf;
use tokio::sync::RwLock;
use tonic::metadata::MetadataValue;
use tonic::Request;

use crate::gcs_client::auth::GcsAuth;
use crate::gcs_client::grpc_client::{
    BidiWriteObjectStream, GcsConnector, GcsGrpcClient, GcsGrpcClientWrapper, WriteObjectStream,
};

#[derive(Clone)]
pub struct ObjectPath {
    bucket: String,
    pub(crate) path: String,
}

impl ObjectPath {
    pub fn new(bucket: String, path: &str) -> Self {
        let normalized_path = path.replace('\\', "/").trim_start_matches('/').to_string();
        Self {
            bucket,
            path: normalized_path,
        }
    }

    pub fn get_formatted_bucket(&self) -> String {
        format!("projects/_/buckets/{}", self.bucket)
    }
}

pub struct GcsClient {
    client: Arc<RwLock<GcsGrpcClientWrapper>>,
    auth: Arc<GcsAuth>,
}

impl GcsClient {
    pub async fn new(
        spec: &GcsSpec,
        jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Result<Self, Error> {
        let connector = GcsConnector::new(spec, jitter_fn.clone());
        let channel_permit = connector.create_channel_with_permit(spec).await?;
        let client_wrapper = GcsGrpcClientWrapper::new(&channel_permit);

        Ok(Self {
            client: Arc::new(RwLock::new(client_wrapper)),
            auth: Arc::new(GcsAuth::new(spec).await?),
        })
    }

    pub async fn new_with_client(
        client: impl GcsGrpcClient + 'static,
        spec: &GcsSpec,
    ) -> Result<Self, Error> {
        let client_wrapper = GcsGrpcClientWrapper::new_with_client(client);

        Ok(Self {
            client: Arc::new(RwLock::new(client_wrapper)),
            auth: Arc::new(GcsAuth::new(spec).await?),
        })
    }

    async fn add_auth_and_common_headers(
        &self,
        metadata: &mut tonic::metadata::MetadataMap,
        object: &ObjectPath,
    ) -> Result<(), Error> {
        let token = self.auth.get_valid_token().await?;
        metadata.insert(
            "Authorization",
            MetadataValue::try_from(&format!("Bearer {token}"))
                .map_err(|e| make_err!(Code::Internal, "Failed to create auth header: {e:?}"))?,
        );

        let bucket = object.get_formatted_bucket();
        let encoded_bucket = urlencoding::encode(&bucket);
        let params = format!("bucket={encoded_bucket}");

        // NOTE: Please don't remove this metadata as it is required for all the requests.
        // I can't find the direct reference for where is it mentioned in the docs, that this
        // is required, but without this metadata, we can't make any requests.
        metadata.insert(
            "x-goog-request-params",
            MetadataValue::try_from(&params)
                .map_err(|e| make_err!(Code::Internal, "Failed to create params header: {e:?}"))?,
        );

        Ok(())
    }

    pub async fn simple_upload(
        &self,
        object: &ObjectPath,
        content: Vec<u8>,
        size: i64,
    ) -> Result<(), Error> {
        let write_spec = WriteObjectSpec {
            resource: Some(Object {
                name: object.path.clone(),
                bucket: object.get_formatted_bucket(),
                size,
                content_type: "application/octet-stream".to_string(),
                ..Default::default()
            }),
            object_size: None,
            ..Default::default()
        };

        let write_stream: WriteObjectStream = Box::pin(futures::stream::iter([
            WriteObjectRequest {
                first_message: Some(write_object_request::FirstMessage::WriteObjectSpec(
                    write_spec,
                )),
                write_offset: 0,
                data: None,
                finish_write: false,
                ..Default::default()
            },
            WriteObjectRequest {
                first_message: None,
                write_offset: 0,
                data: Some(write_object_request::Data::ChecksummedData(
                    ChecksummedData {
                        content: content.clone(),
                        crc32c: Some(crc32c::crc32c(&content)),
                    },
                )),
                finish_write: true,
                ..Default::default()
            },
        ]));

        let client_guard = self.client.write().await;
        let mut request = Request::new(write_stream);
        self.add_auth_and_common_headers(request.metadata_mut(), object)
            .await?;

        client_guard
            .handle_request(|client| Box::pin(client.write_object(request)))
            .await?;

        Ok(())
    }

    pub async fn read_object_metadata(&self, object: ObjectPath) -> Result<Option<Object>, Error> {
        let request = ReadObjectRequest {
            bucket: object.get_formatted_bucket(),
            object: object.path.clone(),
            read_offset: 0,
            read_limit: 0,
            ..Default::default()
        };

        let client_guard = self.client.write().await;
        let mut req = Request::new(request);
        self.add_auth_and_common_headers(req.metadata_mut(), &object)
            .await?;

        match client_guard
            .handle_request(|client| Box::pin(client.read_object(req)))
            .await
        {
            Ok(response) => {
                let mut stream = response.into_inner();
                if let Some(Ok(first_message)) = stream.next().await {
                    Ok(first_message.metadata)
                } else {
                    Ok(None)
                }
            }
            Err(e) if e.code == Code::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn read_object_content(
        &self,
        object: ObjectPath,
        start: i64,
        end: Option<i64>,
    ) -> Result<Vec<u8>, Error> {
        let request = ReadObjectRequest {
            bucket: object.get_formatted_bucket(),
            object: object.path.clone(),
            read_offset: start,
            read_limit: end.map_or(0, |e| e - start),
            ..Default::default()
        };

        let client_guard = self.client.write().await;
        let mut req = Request::new(request);
        self.add_auth_and_common_headers(req.metadata_mut(), &object)
            .await?;

        let response = client_guard
            .handle_request(|client| Box::pin(client.read_object(req)))
            .await
            .map_err(|e| {
                if e.code == Code::NotFound {
                    make_err!(Code::NotFound, "Object not found: {}", object.path)
                } else {
                    e
                }
            })?;

        let mut content = Vec::new();
        let mut stream = response.into_inner();

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(data) => {
                    if let Some(checksummed_data) = data.checksummed_data {
                        content.extend(checksummed_data.content);
                    }
                }
                Err(e) => {
                    return Err(make_err!(
                        Code::Aborted,
                        "Failed to read object chunk: {:?}",
                        e
                    ));
                }
            }
        }

        Ok(content)
    }

    pub async fn start_resumable_write(&self, object: &ObjectPath) -> Result<String, Error> {
        let write_spec = WriteObjectSpec {
            resource: Some(Object {
                name: object.path.clone(),
                bucket: object.get_formatted_bucket(),
                size: -1,
                content_type: "application/octet-stream".to_string(),
                ..Default::default()
            }),
            object_size: None,
            ..Default::default()
        };

        let start_request = StartResumableWriteRequest {
            write_object_spec: Some(write_spec),
            common_object_request_params: None,
            object_checksums: None,
        };

        let client_guard = self.client.write().await;
        let mut request = Request::new(start_request);
        self.add_auth_and_common_headers(request.metadata_mut(), object)
            .await?;

        let response = client_guard
            .handle_request(|client| Box::pin(client.start_resumable_write(request)))
            .await?;

        Ok(response.into_inner().upload_id)
    }

    pub async fn resumable_upload(
        &self,
        object: &ObjectPath,
        reader: &mut DropCloserReadHalf,
        upload_id: &str,
        max_size: i64,
        max_concurrent_uploads: usize,
    ) -> Result<(), Error> {
        use futures::channel::mpsc;
        use futures::SinkExt;

        let client_guard = self.client.write().await;
        let (mut tx, rx) = mpsc::channel(max_concurrent_uploads);

        // Start background chunk processing
        let process_future = {
            let upload_id = upload_id.to_string();
            async move {
                let stream = futures::stream::try_unfold(
                    (reader, 0i64),
                    move |(reader, offset)| async move {
                        match reader.next().await {
                            Some(Ok(chunk)) => {
                                if chunk.is_empty() {
                                    return Ok(None);
                                }

                                let new_offset = offset + chunk.len() as i64;
                                if new_offset > max_size {
                                    return Err(make_err!(
                                        Code::InvalidArgument,
                                        "Upload exceeds maximum size of {}",
                                        max_size
                                    ));
                                }

                                Ok(Some((chunk, (reader, new_offset))))
                            }
                            Some(Err(e)) => Err(make_err!(Code::Internal, "Read error: {}", e)),
                            None => Ok(None),
                        }
                    },
                );

                // Send initial request
                tx.send(BidiWriteObjectRequest {
                    first_message: Some(bidi_write_object_request::FirstMessage::UploadId(
                        upload_id,
                    )),
                    write_offset: 0,
                    finish_write: false,
                    data: None,
                    ..Default::default()
                })
                .await
                .map_err(|e| make_err!(Code::Internal, "Channel send error: {}", e))?;

                // Process chunks
                let mut current_offset = 0i64;
                futures::pin_mut!(stream);
                while let Some(chunk) = stream.try_next().await? {
                    current_offset += chunk.len() as i64;

                    // Create and send chunk request
                    tx.send(BidiWriteObjectRequest {
                        first_message: None,
                        write_offset: current_offset - chunk.len() as i64,
                        data: Some(bidi_write_object_request::Data::ChecksummedData(
                            ChecksummedData {
                                content: chunk.to_vec(),
                                crc32c: Some(crc32c::crc32c(&chunk)),
                            },
                        )),
                        finish_write: false,
                        ..Default::default()
                    })
                    .await
                    .map_err(|e| make_err!(Code::Internal, "Channel send error: {}", e))?;
                }

                // Send final request
                tx.send(BidiWriteObjectRequest {
                    first_message: None,
                    write_offset: current_offset,
                    data: None,
                    finish_write: true,
                    ..Default::default()
                })
                .await
                .map_err(|e| make_err!(Code::Internal, "Channel send error: {}", e))?;

                Ok(())
            }
        };

        // Create request stream
        let request_stream = Box::pin(rx) as BidiWriteObjectStream;
        let mut request = Request::new(request_stream);
        self.add_auth_and_common_headers(request.metadata_mut(), object)
            .await?;

        // Upload chunks while processing more
        let (upload_result, process_result) = tokio::join!(
            async {
                let response = client_guard
                    .handle_request(|client| Box::pin(client.bidi_write_object(request)))
                    .await?;

                let mut stream = response.into_inner();
                while let Some(response) = stream.next().await {
                    response?;
                }
                Ok::<_, Error>(())
            },
            process_future
        );

        upload_result.and(process_result)
    }
}
