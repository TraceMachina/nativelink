// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::fmt::Debug;

use async_trait::async_trait;

use tokio::io::{AsyncRead, Error};

#[async_trait]
pub trait StoreTrait: Sync + Send + Debug {
    fn has(&self, hash: &str) -> bool;
    async fn update<'a>(
        &'a self,
        _hash: &'a str,
        _size: i64,
        _reader: Box<dyn AsyncRead + Send>,
    ) -> Result<(), Error>;
}
