// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::marker::Send;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Future;
use tokio::io::{AsyncRead, AsyncWrite};

use common::DigestInfo;
use error::Error;

pub enum StoreType {
    Memory,
}

pub type ResultFuture<'a, Res> = Pin<Box<dyn Future<Output = Result<Res, Error>> + 'a + Sync + Send>>;

#[async_trait]
pub trait StoreTrait: Sync + Send + Unpin {
    fn has<'a>(self: Pin<&'a Self>, digest: DigestInfo) -> ResultFuture<'a, bool>;

    fn update<'a>(
        self: Pin<&'a Self>,
        digest: DigestInfo,
        reader: Box<dyn AsyncRead + Send + Unpin + Sync + 'a>,
    ) -> ResultFuture<'a, ()>;

    fn get_part<'a>(
        self: Pin<&'a Self>,
        digest: DigestInfo,
        writer: &'a mut (dyn AsyncWrite + Send + Unpin + Sync),
        offset: usize,
        length: Option<usize>,
    ) -> ResultFuture<'a, ()>;

    fn get<'a>(
        self: Pin<&'a Self>,
        digest: DigestInfo,
        writer: &'a mut (dyn AsyncWrite + Send + Unpin + Sync),
    ) -> ResultFuture<'a, ()> {
        self.get_part(digest, writer, 0, None)
    }
}
