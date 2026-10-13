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
use tracing::{error, info, warn};

use crate::metrics::record_retry_outcome;

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

/// What became of an operation, or one attempt of it, under a [`Retrier`].
/// Recorded as `retry.outcomes`, labeled with the retrier's client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOutcome {
    /// An attempt failed with a retryable error and another follows.
    Retried,
    /// The operation succeeded after at least one retry.
    Recovered,
    /// The operation failed with a retryable error and the retries ran out.
    Exhausted,
    /// The operation failed with an error that is never retried. `NotFound`
    /// is left out: on a store it is the ordinary miss.
    Permanent,
}

impl RetryOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Retried => "retried",
            Self::Recovered => "recovered",
            Self::Exhausted => "exhausted",
            Self::Permanent => "permanent",
        }
    }
}

type OutcomeFn = Arc<dyn Fn(RetryOutcome) + Send + Sync>;

/// Class used to retry a job with a sleep function in between each retry.
#[derive(Clone)]
pub struct Retrier {
    sleep_fn: SleepFn,
    jitter_fn: JitterFn,
    config: Retry,
    client: &'static str,
    outcome_fn: Option<OutcomeFn>,
}

impl core::fmt::Debug for Retrier {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Retrier")
            .field("config", &self.config)
            .field("client", &self.client)
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
            client: "unknown",
            outcome_fn: None,
        }
    }

    /// Names what this retrier retries for (`azure`, `s3`, `grpc_store`,
    /// ...), the `retry.client` label on its `retry.outcomes`.
    #[must_use]
    pub const fn with_client(mut self, client: &'static str) -> Self {
        self.client = client;
        self
    }

    /// Also hands every outcome to `outcome_fn`, after it is recorded as a
    /// metric. For tests, which cannot read the meter.
    #[must_use]
    pub fn with_outcome_fn(mut self, outcome_fn: OutcomeFn) -> Self {
        self.outcome_fn = Some(outcome_fn);
        self
    }

    fn report(&self, outcome: RetryOutcome) {
        record_retry_outcome(self.client, outcome.as_str());
        if let Some(outcome_fn) = &self.outcome_fn {
            outcome_fn(outcome);
        }
    }

    /// This should only return true if the error code should be interpreted as
    /// temporary.
    fn should_retry(&self, code: Code) -> bool {
        if let Some(retry_codes) = &self.config.retry_on_errors {
            retry_codes.contains(&to_error_code(code))
        } else {
            // Match is intentionally exhaustive: if a new `Code` variant is added,
            // the compiler forces a decision here. New codes default to permanent
            // (no retry) rather than silently being retried.
            match code {
                Code::Cancelled
                | Code::Unknown
                | Code::DeadlineExceeded
                | Code::ResourceExhausted
                | Code::Aborted
                | Code::Internal
                | Code::Unavailable
                | Code::DataLoss => true,
                Code::Ok
                | Code::InvalidArgument
                | Code::NotFound
                | Code::AlreadyExists
                | Code::PermissionDenied
                | Code::FailedPrecondition
                | Code::OutOfRange
                | Code::Unimplemented
                | Code::Unauthenticated => false,
            }
        }
    }

    fn get_retry_config(&self) -> impl Iterator<Item = Duration> + '_ {
        ExponentialBackoff::new(Duration::from_secs_f32(self.config.delay))
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
                    Some(RetryResult::Ok(value)) => {
                        if attempt > 1 {
                            self.report(RetryOutcome::Recovered);
                        }
                        return Ok(value);
                    }
                    Some(RetryResult::Err(e)) => {
                        return Err(e.append(format!("On attempt {attempt}")));
                    }
                    Some(RetryResult::Retry(err)) => {
                        if !self.should_retry(err.code) {
                            if err.code == Code::NotFound {
                                info!(?err, "Not found, not retrying");
                            } else {
                                self.report(RetryOutcome::Permanent);
                                error!(?attempt, ?err, "Not retrying permanent error");
                            }
                            return Err(err);
                        }
                        let Some(delay) = iter.next() else {
                            self.report(RetryOutcome::Exhausted);
                            warn!(client = self.client, ?attempt, ?err, "Retries exhausted");
                            return Err(err.append(format!("On attempt {attempt}")));
                        };
                        self.report(RetryOutcome::Retried);
                        (self.sleep_fn)(delay).await;
                    }
                }
            }
        }
    }
}
