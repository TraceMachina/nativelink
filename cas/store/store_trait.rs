// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use common::DigestInfo;
use error::Error;

#[async_trait]
pub trait StoreTrait: Sync + Send {
    async fn has(&self, digest: &DigestInfo) -> Result<bool, Error>;

    async fn update<'a, 'b>(
        &'a self,
        digest: &'a DigestInfo,
        mut reader: Box<dyn AsyncRead + Send + Unpin + 'b>,
    ) -> Result<(), Error>;

    async fn get(
        &self,
        digest: &DigestInfo,
        writer: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> Result<(), Error>;
}
