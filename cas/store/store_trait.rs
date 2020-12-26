// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::fmt::Debug;

use async_trait::async_trait;

use tokio::io::{AsyncRead, AsyncWrite, Error};

#[async_trait]
pub trait StoreTrait: Sync + Send + Debug {
    async fn has(&self, hash: &str, expected_size: usize) -> Result<bool, Error>;
    async fn update<'a, 'b>(
        &'a self,
        hash: &'a str,
        expected_size: usize,
        mut _reader: Box<dyn AsyncRead + Send + Unpin + 'b>,
    ) -> Result<(), Error>;

    async fn get(
        &self,
        hash: &str,
        expected_size: usize,
        writer: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> Result<(), Error>;
}
