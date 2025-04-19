// azure_store.rs
use azure_storage::client::StorageClient;
use azure_storage::blob::{BlobServiceClient, BlobContainerClient, BlobClient};

pub struct AzureBlobStore {
    container_client: BlobContainerClient,
}

impl AzureBlobStore {
    pub fn new(account_name: &str, account_key: &str, container_name: &str) -> Self {
        let blob_service_client = BlobServiceClient::new_with_account_and_key(account_name, account_key);
        let container_client = blob_service_client.as_container_client(container_name);
        AzureBlobStore { container_client }
    }

    pub async fn upload(&self, blob_name: &str, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let blob_client = self.container_client.as_blob_client(blob_name);
        blob_client.put_block_blob(data).await?;
        Ok(())
    }

    pub async fn download(&self, blob_name: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let blob_client = self.container_client.as_blob_client(blob_name);
        let blob = blob_client.download().await?;
        Ok(blob.body())
    }

    pub async fn list_blobs(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let blobs = self.container_client.list_blobs().await?;
        let blob_names = blobs.iter().map(|b| b.name.clone()).collect();
        Ok(blob_names)
    }
}



