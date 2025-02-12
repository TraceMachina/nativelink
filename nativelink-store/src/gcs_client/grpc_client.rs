use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::unfold;
use futures::Stream;
use googleapis_tonic_google_storage_v2::google::storage::v2::storage_client::StorageClient;
use googleapis_tonic_google_storage_v2::google::storage::v2::{
    BidiWriteObjectRequest, BidiWriteObjectResponse, ReadObjectRequest, ReadObjectResponse,
    StartResumableWriteRequest, StartResumableWriteResponse, WriteObjectRequest,
    WriteObjectResponse,
};
use nativelink_config::stores::GcsSpec;
use nativelink_error::{make_err, Code, Error};
use nativelink_util::fs;
use nativelink_util::retry::{Retrier, RetryResult};
use tokio::sync::SemaphorePermit;
use tokio::time::sleep;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Response, Status, Streaming};

pub struct ChannelWithPermit {
    channel: Channel,
    _permit: SemaphorePermit<'static>,
}

impl ChannelWithPermit {
    pub fn get_channel(&self) -> Channel {
        self.channel.clone()
    }
}

#[derive(Clone)]
pub struct GcsConnector {
    retrier: Retrier,
}

impl GcsConnector {
    #[must_use]
    pub fn new(spec: &GcsSpec, jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>) -> Self {
        Self {
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                spec.retry.clone(),
            ),
        }
    }

    fn create_endpoint(&self, spec: &GcsSpec) -> Result<Endpoint, Error> {
        let endpoint = match &spec.endpoint {
            Some(endpoint) => format!("https://{endpoint}"),
            None => std::env::var("GOOGLE_STORAGE_ENDPOINT")
                .unwrap_or_else(|_| "https://storage.googleapis.com".to_string()),
        };

        let channel = Channel::from_shared(endpoint)
            .map_err(|e| make_err!(Code::InvalidArgument, "Invalid GCS endpoint: {e:?}"))?
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .tcp_nodelay(true)
            .http2_adaptive_window(true)
            .http2_keep_alive_interval(Duration::from_secs(30))
            .tls_config(ClientTlsConfig::new().with_native_roots())
            .map_err(|e| make_err!(Code::InvalidArgument, "Failed to configure TLS: {e:?}"))?;

        Ok(channel)
    }

    async fn connect_with_permit(&self, endpoint: Endpoint) -> Result<ChannelWithPermit, Error> {
        self.retrier
            .retry(unfold(endpoint, |endpoint| async move {
                // Get a permit before establishing connection
                let permit = match fs::get_permit().await {
                    Ok(permit) => permit,
                    Err(e) => return Some((RetryResult::Retry(e), endpoint)),
                };

                match endpoint.connect().await {
                    Ok(channel) => Some((
                        RetryResult::Ok(ChannelWithPermit {
                            channel,
                            _permit: permit,
                        }),
                        endpoint,
                    )),
                    Err(e) => Some((
                        RetryResult::Retry(make_err!(
                            Code::Unavailable,
                            "Failed to connect to GCS: {e:?}"
                        )),
                        endpoint,
                    )),
                }
            }))
            .await
    }

    pub async fn create_channel_with_permit(
        &self,
        spec: &GcsSpec,
    ) -> Result<ChannelWithPermit, Error> {
        let endpoint = self.create_endpoint(spec)?;
        self.connect_with_permit(endpoint).await
    }
}

pub type WriteObjectStream = Pin<Box<dyn Stream<Item = WriteObjectRequest> + Send + 'static>>;
pub type BidiWriteObjectStream =
    Pin<Box<dyn Stream<Item = BidiWriteObjectRequest> + Send + 'static>>;

/// Trait defining the GCS gRPC operations
#[async_trait::async_trait]
pub trait GcsGrpcClient: Send + Sync + 'static {
    /// Read an object from GCS with streaming response
    async fn read_object(
        &mut self,
        request: Request<ReadObjectRequest>,
    ) -> Result<Response<Streaming<ReadObjectResponse>>, Status>;

    /// Write an object to GCS with streaming request
    async fn write_object(
        &mut self,
        request: Request<WriteObjectStream>,
    ) -> Result<Response<WriteObjectResponse>, Status>;

    /// Start a resumable upload session
    async fn start_resumable_write(
        &mut self,
        request: Request<StartResumableWriteRequest>,
    ) -> Result<Response<StartResumableWriteResponse>, Status>;

    /// Bidirectional streaming upload
    async fn bidi_write_object(
        &mut self,
        request: Request<BidiWriteObjectStream>,
    ) -> Result<Response<Streaming<BidiWriteObjectResponse>>, Status>;
}

#[async_trait::async_trait]
impl GcsGrpcClient for StorageClient<Channel> {
    #[inline]
    async fn read_object(
        &mut self,
        request: Request<ReadObjectRequest>,
    ) -> Result<Response<Streaming<ReadObjectResponse>>, Status> {
        StorageClient::read_object(self, request).await
    }

    #[inline]
    async fn write_object(
        &mut self,
        request: Request<WriteObjectStream>,
    ) -> Result<Response<WriteObjectResponse>, Status> {
        let fut: Pin<
            Box<dyn Future<Output = Result<Response<WriteObjectResponse>, Status>> + Send>,
        > = Box::pin(StorageClient::write_object(self, request));
        fut.await
    }

    #[inline]
    async fn start_resumable_write(
        &mut self,
        request: Request<StartResumableWriteRequest>,
    ) -> Result<Response<StartResumableWriteResponse>, Status> {
        StorageClient::start_resumable_write(self, request).await
    }

    #[inline]
    async fn bidi_write_object(
        &mut self,
        request: Request<BidiWriteObjectStream>,
    ) -> Result<Response<Streaming<BidiWriteObjectResponse>>, Status> {
        let fut: Pin<
            Box<
                dyn Future<Output = Result<Response<Streaming<BidiWriteObjectResponse>>, Status>>
                    + Send,
            >,
        > = Box::pin(StorageClient::bidi_write_object(self, request));
        fut.await
    }
}

pub struct GcsGrpcClientWrapper {
    inner: Arc<tokio::sync::RwLock<Box<dyn GcsGrpcClient>>>,
}

impl GcsGrpcClientWrapper {
    pub fn new(channel_permit: &ChannelWithPermit) -> Self {
        let client = StorageClient::new(channel_permit.get_channel());
        Self {
            inner: Arc::new(tokio::sync::RwLock::new(Box::new(client))),
        }
    }

    pub fn new_with_client(client: impl GcsGrpcClient + 'static) -> Self {
        Self {
            inner: Arc::new(tokio::sync::RwLock::new(Box::new(client))),
        }
    }

    #[inline]
    pub async fn handle_request<T>(
        &self,
        operation: impl FnOnce(
            &mut dyn GcsGrpcClient,
        )
            -> Pin<Box<dyn Future<Output = Result<Response<T>, Status>> + Send + '_>>,
    ) -> Result<Response<T>, Error> {
        let mut guard = self.inner.write().await;
        operation(&mut **guard).await.map_err(|e| match e.code() {
            tonic::Code::NotFound => make_err!(Code::NotFound, "Resource not found: {}", e),
            tonic::Code::Unavailable => make_err!(Code::Unavailable, "Service unavailable: {}", e),
            tonic::Code::Internal => make_err!(Code::Internal, "Internal error: {}", e),
            _ => make_err!(Code::Unavailable, "Operation failed: {}", e),
        })
    }
}
