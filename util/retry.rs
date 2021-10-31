// Copyright 2021 Nathan (Blaise) Bruer.  All rights reserved.

use futures::future::Future;
use futures::stream::StreamExt;
use std::pin::Pin;
use std::time::Duration;

use error::{make_err, Code, Error};

pub struct ExponentialBackoff {
    current: Duration,
}

impl ExponentialBackoff {
    pub fn new(base: Duration) -> Self {
        ExponentialBackoff { current: base }
    }
}

impl Iterator for ExponentialBackoff {
    type Item = Duration;

    fn next(&mut self) -> Option<Duration> {
        self.current = self.current * 2;
        Some(self.current)
    }
}

type SleepFn = Box<dyn Fn(Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> + Sync + Send>;

#[derive(PartialEq, Eq, Debug)]
pub enum RetryResult<T> {
    Ok(T),
    Retry(Error),
    Err(Error),
}

/// Class used to retry a job with a sleep function in between each retry.
pub struct Retrier {
    sleep_fn: SleepFn,
}

impl Retrier {
    pub fn new(sleep_fn: SleepFn) -> Self {
        Retrier { sleep_fn }
    }

    pub fn retry<'a, T, Fut, I>(
        &'a self,
        duration_iter: I,
        operation: Fut,
    ) -> Pin<Box<dyn Future<Output = Result<T, Error>> + 'a + Send>>
    where
        Fut: futures::stream::Stream<Item = RetryResult<T>> + Send + 'a,
        I: IntoIterator<Item = Duration> + Send + 'a,
        <I as IntoIterator>::IntoIter: Send,
        T: Send,
    {
        Box::pin(async move {
            let mut iter = duration_iter.into_iter();
            let mut operation = Box::pin(operation);
            loop {
                match operation.next().await {
                    None => return Err(make_err!(Code::Internal, "Retry stream ended abruptly",)),
                    Some(RetryResult::Ok(value)) => return Ok(value),
                    Some(RetryResult::Err(e)) => return Err(e),
                    Some(RetryResult::Retry(e)) => (self.sleep_fn)(iter.next().ok_or_else(|| e)?).await,
                }
            }
        })
    }
}
