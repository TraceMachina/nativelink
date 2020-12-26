// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::fmt::Debug;

use async_trait::async_trait;

use tokio::io::{AsyncRead, Error};

#[async_trait]
pub trait StoreTrait: Sync + Send + Debug {
    async fn has(&self, hash: &str) -> Result<bool, Error>;
    async fn update<'a>(
        &'a mut self,
        _hash: &'a str,
        _size: usize,
        mut _reader: Box<dyn AsyncRead + Send + Unpin>,
    ) -> Result<(), Error>;
}
