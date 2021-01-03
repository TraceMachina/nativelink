// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use common::DigestInfo;
use error::Error;

pub enum StoreType {
    Memory,
}

pub struct StoreConfig {
    pub store_type: StoreType,

    // If we need to verify the digest size of what is being uploaded.
    pub verify_size: bool,
}

#[async_trait]
pub trait StoreTrait: Sync + Send {
    async fn has(&self, digest: &DigestInfo) -> Result<bool, Error>;

    async fn update<'a, 'b>(
        &'a self,
        digest: &'a DigestInfo,
        mut reader: Box<dyn AsyncRead + Send + Unpin + 'b>,
    ) -> Result<(), Error>;

    async fn get_part(
        &self,
        digest: &DigestInfo,
        writer: &mut (dyn AsyncWrite + Send + Unpin),
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error>;

    async fn get(
        &self,
        digest: &DigestInfo,
        writer: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> Result<(), Error> {
        self.get_part(digest, writer, 0, None).await
    }
}
