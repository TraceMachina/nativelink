// Copyright 2023 The Native Link Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use error::{make_err, Code, Error};
use futures::future::Future;
use futures::stream::StreamExt;

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
        self.current *= 2;
        Some(self.current)
    }
}

type SleepFn = Arc<dyn Fn(Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> + Sync + Send>;

#[derive(PartialEq, Eq, Debug)]
pub enum RetryResult<T> {
    Ok(T),
    Retry(Error),
    Err(Error),
}

/// Class used to retry a job with a sleep function in between each retry.
#[derive(Clone)]
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
                    Some(RetryResult::Retry(e)) => (self.sleep_fn)(iter.next().ok_or(e)?).await,
                }
            }
        })
    }
}
