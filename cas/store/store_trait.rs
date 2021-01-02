// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use error::Error;

#[async_trait]
pub trait StoreTrait: Sync + Send {
    async fn has(&self, hash: &str, expected_size: usize) -> Result<bool, Error>;

    async fn update<'a, 'b>(
        &'a self,
        hash: &'a str,
        expected_size: usize,
        mut reader: Box<dyn AsyncRead + Send + Unpin + 'b>,
    ) -> Result<(), Error>;

    async fn get(
        &self,
        hash: &str,
        expected_size: usize,
        writer: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> Result<(), Error>;
}
