// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::fmt::Debug;

use async_trait::async_trait;

use tokio::io::{AsyncRead, AsyncWrite, Error};

#[async_trait]
pub trait StoreTrait: Sync + Send + Debug {
    async fn has(&self, hash: &str, _expected_size: usize) -> Result<bool, Error>;
    async fn update<'a>(
        &'a mut self,
        _hash: &'a str,
        _expected_size: usize,
        mut _reader: Box<dyn AsyncRead + Send + Unpin>,
    ) -> Result<(), Error>;

    async fn get(
        &self,
        hash: &str,
        _expected_size: usize,
        writer: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> Result<(), Error>;
}
