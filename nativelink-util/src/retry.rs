// Copyright 2023-2024 The Native Link Authors. All rights reserved.
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

use futures::future::Future;
use futures::stream::StreamExt;
use nativelink_config::stores::Retry;
use nativelink_error::{make_err, Code, Error};
use tracing::debug;

struct ExponentialBackoff {
    current: Duration,
}

impl ExponentialBackoff {
    fn new(base: Duration) -> Self {
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
type JitterFn = Arc<dyn Fn(Duration) -> Duration + Send + Sync>;

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
    jitter_fn: JitterFn,
    config: Retry,
}

impl Retrier {
    pub fn new(sleep_fn: SleepFn, jitter_fn: JitterFn, config: Retry) -> Self {
        Retrier {
            sleep_fn,
            jitter_fn,
            config,
        }
    }

    /// This should only return true if the error code should be interpreted as
    /// temporary.
    fn should_retry(&self, code: &Code) -> bool {
        match code {
            Code::Ok => false,
            Code::InvalidArgument => false,
            Code::FailedPrecondition => false,
            Code::OutOfRange => false,
            Code::Unimplemented => false,
            Code::NotFound => self.config.retry_mapping.not_found,
            Code::AlreadyExists => self.config.retry_mapping.already_exists,
            Code::PermissionDenied => self.config.retry_mapping.permission_denied,
            Code::Unauthenticated => self.config.retry_mapping.unauthenticated,
            Code::Cancelled => self.config.retry_mapping.other,
            Code::Unknown => self.config.retry_mapping.other,
            Code::DeadlineExceeded => self.config.retry_mapping.other,
            Code::ResourceExhausted => self.config.retry_mapping.other,
            Code::Aborted => self.config.retry_mapping.other,
            Code::Internal => self.config.retry_mapping.other,
            Code::Unavailable => self.config.retry_mapping.other,
            Code::DataLoss => self.config.retry_mapping.other,
            _ => self.config.retry_mapping.other,
        }
    }

    fn get_retry_config(&self) -> impl Iterator<Item = Duration> + '_ {
        ExponentialBackoff::new(Duration::from_millis(self.config.delay as u64))
            .map(|d| (self.jitter_fn)(d))
            .take(self.config.max_retries) // Remember this is number of retries, so will run max_retries + 1.
    }

    pub fn retry<'a, T, Fut>(&'a self, operation: Fut) -> Pin<Box<dyn Future<Output = Result<T, Error>> + 'a + Send>>
    where
        Fut: futures::stream::Stream<Item = RetryResult<T>> + Send + 'a,
        T: Send,
    {
        Box::pin(async move {
            let mut iter = self.get_retry_config();
            let mut operation = Box::pin(operation);
            let mut attempt = 0;
            loop {
                attempt += 1;
                match operation.next().await {
                    None => {
                        return Err(make_err!(
                            Code::Internal,
                            "Retry stream ended abruptly on attempt {attempt}",
                        ))
                    }
                    Some(RetryResult::Ok(value)) => return Ok(value),
                    Some(RetryResult::Err(e)) => return Err(e.append(format!("On attempt {attempt}"))),
                    Some(RetryResult::Retry(e)) => {
                        if !self.should_retry(&e.code) {
                            debug!("Not retrying permanent error on attempt {attempt}: {e:?}");
                            return Err(e);
                        }
                        (self.sleep_fn)(iter.next().ok_or(e.append(format!("On attempt {attempt}")))?).await
                    }
                }
            }
        })
    }
}
