// azure_store.rs

use std::borrow::Cow;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use azure_storage::core::prelude::*;
use azure_storage_blobs::prelude::*;
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use nativelink_config::stores::AzureBlobSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};

#[derive(Debug, MetricsComponent)]
pub struct AzureBlobStore<NowFn> {
    container_client: ContainerClient,
    now_fn: NowFn,
    #[metric(help = "The Azure Blob container name")]
    container: String,
    #[metric(help = "The blob key prefix")]
    blob_prefix: String,
    retrier: Retrier,
    #[metric(help = "Maximum buffer size per request for retrying")]
    max_retry_buffer_per_request: usize,
}

impl<I, NowFn> AzureBlobStore<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    pub fn new(spec: &AzureBlobSpec, now_fn: NowFn) -> Result<Arc<Self>, Error> {
        let credentials = StorageCredentials::Key(
            spec.account_name.clone(),
            spec.account_key.clone().into(),
        );
        let blob_service_client = BlobServiceClient::new(
            StorageAccountClient::new(credentials),
        );
        let container_client = blob_service_client.container_client(&spec.container);

        let retrier = Retrier::new(
            Arc::new(|duration| Box::pin(tokio::time::sleep(duration))),
            Arc::new(|d| d), // TODO: use jitter_fn if configured
            spec.retry.clone(),
        );

        Ok(Arc::new(Self {
            container_client,
            now_fn,
            container: spec.container.clone(),
            blob_prefix: spec.blob_prefix.clone().unwrap_or_default(),
            retrier,
            max_retry_buffer_per_request: spec
                .max_retry_buffer_per_request
                .unwrap_or(5 * 1024 * 1024),
        }))
    }

    fn make_blob_path(&self, key: &StoreKey<'_>) -> String {
        format!("{}{}", self.blob_prefix, key.as_str())
    }
}

#[async_trait]
impl<I, NowFn> StoreDriver for AzureBlobStore<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        for (key, result) in keys.iter().zip(results.iter_mut()) {
            let blob_name = self.make_blob_path(key);
            let exists = self
                .container_client
                .blob_client(blob_name)
                .get_properties()
                .await
                .map(|props| Some(props.blob.properties.content_length))
                .unwrap_or(None);
            *result = exists;
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let blob_name = self.make_blob_path(&key);
        let blob_client = self.container_client.blob_client(blob_name);

        let mut data = Vec::new();
        while let Some(chunk) = reader.next().await.transpose()? {
            data.extend_from_slice(&chunk);
        }

        blob_client
            .put_block_blob(data.clone())
            .into_future()
            .await
            .map_err(|e| make_err!(Code::Aborted, "Azure put_blob error: {e}"))?;

        Ok(())
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let blob_name = self.make_blob_path(&key);
        let blob_client = self.container_client.blob_client(blob_name);
        let range = offset..offset + length.unwrap_or(u64::MAX - offset);

        let response = blob_client
            .get()
            .range(range.clone())
            .into_stream()
            .next()
            .await
            .ok_or_else(|| make_err!(Code::Unavailable, "No Azure blob response"))
            .and_then(|r| r.map_err(|e| make_err!(Code::Unavailable, "Azure get_blob error: {e}")))?;

        writer.send(Bytes::from(response.data)).await?;
        writer.send_eof()?;
        Ok(())
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &'_ dyn StoreDriver {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn std::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }
}

#[async_trait]
impl<I, NowFn> HealthStatusIndicator for AzureBlobStore<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    fn get_name(&self) -> &'static str {
        "AzureBlobStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}


