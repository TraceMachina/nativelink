// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::pin::Pin;
use core::time::Duration;
use std::sync::Arc;

use futures::future::Future;
use futures::stream::StreamExt;
use nativelink_config::stores::{ErrorCode, Retry};
use nativelink_error::{Code, Error, make_err};
use tracing::error;

struct ExponentialBackoff {
    current: Duration,
}

impl ExponentialBackoff {
    const fn new(base: Duration) -> Self {
        Self { current: base }
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
pub(crate) type JitterFn = Arc<dyn Fn(Duration) -> Duration + Send + Sync>;

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

impl core::fmt::Debug for Retrier {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Retrier")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

const fn to_error_code(code: Code) -> ErrorCode {
    match code {
        Code::Cancelled => ErrorCode::Cancelled,
        Code::InvalidArgument => ErrorCode::InvalidArgument,
        Code::DeadlineExceeded => ErrorCode::DeadlineExceeded,
        Code::NotFound => ErrorCode::NotFound,
        Code::AlreadyExists => ErrorCode::AlreadyExists,
        Code::PermissionDenied => ErrorCode::PermissionDenied,
        Code::ResourceExhausted => ErrorCode::ResourceExhausted,
        Code::FailedPrecondition => ErrorCode::FailedPrecondition,
        Code::Aborted => ErrorCode::Aborted,
        Code::OutOfRange => ErrorCode::OutOfRange,
        Code::Unimplemented => ErrorCode::Unimplemented,
        Code::Internal => ErrorCode::Internal,
        Code::Unavailable => ErrorCode::Unavailable,
        Code::DataLoss => ErrorCode::DataLoss,
        Code::Unauthenticated => ErrorCode::Unauthenticated,
        _ => ErrorCode::Unknown,
    }
}

impl Retrier {
    pub fn new(sleep_fn: SleepFn, jitter_fn: JitterFn, config: Retry) -> Self {
        Self {
            sleep_fn,
            jitter_fn,
            config,
        }
    }

    /// This should only return true if the error code should be interpreted as
    /// temporary.
    fn should_retry(&self, code: Code) -> bool {
        if code == Code::Ok {
            false
        } else if let Some(retry_codes) = &self.config.retry_on_errors {
            retry_codes.contains(&to_error_code(code))
        } else {
            match code {
                // TODO(SchahinRohani): Handle all cases properly so there is no need for a wildcard match
                // All cases where a retry should happen are commented out here and replaced with a wildcard match.
                Code::InvalidArgument
                | Code::FailedPrecondition
                | Code::OutOfRange
                | Code::Unimplemented
                | Code::NotFound
                | Code::AlreadyExists
                | Code::PermissionDenied
                | Code::Unauthenticated => false,
                // Code::Cancelled
                // | Code::Unknown
                // | Code::DeadlineExceeded
                // | Code::ResourceExhausted
                // | Code::Aborted
                // | Code::Internal
                // | Code::Unavailable
                // | Code::DataLoss => true,
                _ => true,
            }
        }
    }

    fn get_retry_config(&self) -> impl Iterator<Item = Duration> + '_ {
        ExponentialBackoff::new(Duration::from_millis(self.config.delay as u64))
            .map(|d| (self.jitter_fn)(d))
            .take(self.config.max_retries) // Remember this is number of retries, so will run max_retries + 1.
    }

    #[expect(
        clippy::manual_async_fn,
        reason = "making an `async fn` results in a potential compiler bug in seemingly unrelated \
            code"
    )]
    pub fn retry<'a, T: Send>(
        &'a self,
        operation: impl futures::stream::Stream<Item = RetryResult<T>> + Send + 'a,
    ) -> impl Future<Output = Result<T, Error>> + Send + 'a {
        async move {
            let mut iter = self.get_retry_config();
            tokio::pin!(operation);
            let mut attempt = 0;
            loop {
                attempt += 1;
                match operation.next().await {
                    None => {
                        return Err(make_err!(
                            Code::Internal,
                            "Retry stream ended abruptly on attempt {attempt}",
                        ));
                    }
                    Some(RetryResult::Ok(value)) => return Ok(value),
                    Some(RetryResult::Err(e)) => {
                        return Err(e.append(format!("On attempt {attempt}")));
                    }
                    Some(RetryResult::Retry(err)) => {
                        if !self.should_retry(err.code) {
                            error!(?attempt, ?err, "Not retrying permanent error");
                            return Err(err);
                        }
                        (self.sleep_fn)(
                            iter.next()
                                .ok_or_else(|| err.append(format!("On attempt {attempt}")))?,
                        )
                        .await;
                    }
                }
            }
        }
    }
}
