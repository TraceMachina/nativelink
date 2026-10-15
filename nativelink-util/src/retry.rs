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
    /// Retryable, and the server asked for at least this long before the
    /// next attempt. Within the retrier's throttle budget the wait is taken
    /// as is and spends no attempt; past it the backoff still applies when
    /// it is longer and the attempt counts.
    RetryAfter(Error, Duration),
    Err(Error),
}

/// What became of an operation, or one attempt of it, under a [`Retrier`].
/// Recorded as `retry.outcomes`, labeled with the retrier's client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOutcome {
    /// An attempt failed with a retryable error and another follows. A
    /// throttle waited out within the throttle budget counts, though it
    /// spends no attempt.
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
    /// How long one `retry` call may spend on `RetryAfter` waits before
    /// they start spending attempts. Zero means they always do.
    throttle_budget: Duration,
    /// Longest a throttle wait may grow to. Set, each consecutive throttle
    /// doubles the wait the store asked for, capped here, and jitters it.
    /// Unset, the wait is what was asked.
    throttle_max_wait: Option<Duration>,
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
            throttle_budget: Duration::ZERO,
            throttle_max_wait: None,
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

    /// Lets one `retry` call spend up to `budget` waiting on `RetryAfter`
    /// results without counting those tries as attempts.
    #[must_use]
    pub const fn with_throttle_budget(mut self, budget: Duration) -> Self {
        self.throttle_budget = budget;
        self
    }

    /// Lets consecutive throttles wait longer each time. The first throttle
    /// in a row waits what the server asked, the second twice that, the
    /// third four times, and so on; `max_wait` is the ceiling that growth
    /// stops at, not a wait of its own. The grown wait is jittered and never
    /// less than what was asked. A store that is told to slow down with no
    /// `Retry-After` otherwise has every client retry in lockstep at the
    /// configured floor.
    #[must_use]
    pub const fn with_throttle_max_wait(mut self, max_wait: Duration) -> Self {
        self.throttle_max_wait = Some(max_wait);
        self
    }

    /// The wait for the `n`-th consecutive throttle whose server asked for
    /// `asked`: `asked` itself for the first, doubled once per throttle
    /// before it, capped at the configured ceiling, then jittered.
    fn throttle_wait(&self, asked: Duration, throttles_in_a_row: u32) -> Duration {
        let Some(max_wait) = self.throttle_max_wait else {
            return asked;
        };
        // One doubling per earlier throttle in the row; past 31 the
        // multiplier would overflow and the ceiling applies anyway.
        let doublings = throttles_in_a_row.saturating_sub(1).min(31);
        let grown = asked.saturating_mul(1 << doublings).min(max_wait);
        // The jitter spreads the grown wait a little either way, as
        // `Retry::make_jitter_fn` does. It may not undo the growth: a
        // result below the ask (a jitter function that zeroes delays, as
        // tests install) leaves the grown wait as it is.
        let jittered = (self.jitter_fn)(grown);
        if jittered < asked { grown } else { jittered }
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
            let mut throttled = Duration::ZERO;
            let mut throttles_in_a_row: u32 = 0;
            loop {
                attempt += 1;
                let (err, min_wait, is_throttle) = match operation.next().await {
                    None => {
                        return Err(make_err!(
                            Code::Internal,
                            "Retry stream ended abruptly on attempt {attempt}",
                        ));
                    }
                    Some(RetryResult::Ok(value)) => {
                        // A throttle within the budget is a retry too,
                        // though it leaves `attempt` where it was.
                        if attempt > 1 || !throttled.is_zero() {
                            self.report(RetryOutcome::Recovered);
                        }
                        return Ok(value);
                    }
                    Some(RetryResult::Err(e)) => {
                        return Err(e.append(format!("On attempt {attempt}")));
                    }
                    Some(RetryResult::Retry(err)) => {
                        throttles_in_a_row = 0;
                        (err, Duration::ZERO, false)
                    }
                    Some(RetryResult::RetryAfter(err, min_wait)) => {
                        throttles_in_a_row = throttles_in_a_row.saturating_add(1);
                        let wait = self.throttle_wait(min_wait, throttles_in_a_row);
                        (err, wait, true)
                    }
                };
                if !self.should_retry(err.code) {
                    if err.code == Code::NotFound {
                        info!(?err, "Not found, not retrying");
                    } else {
                        self.report(RetryOutcome::Permanent);
                        error!(?attempt, ?err, "Not retrying permanent error");
                    }
                    return Err(err);
                }
                // A throttle within the budget waits what was asked and is
                // not an attempt: the server said when to come back, and
                // the budget bounds how long that is listened to. A zero
                // wait would spin, so it goes through the backoff instead.
                if is_throttle
                    && !min_wait.is_zero()
                    && throttled.saturating_add(min_wait) <= self.throttle_budget
                {
                    throttled = throttled.saturating_add(min_wait);
                    attempt -= 1;
                    self.report(RetryOutcome::Retried);
                    (self.sleep_fn)(min_wait).await;
                    continue;
                }
                let Some(backoff) = iter.next() else {
                    self.report(RetryOutcome::Exhausted);
                    warn!(client = self.client, ?attempt, ?err, "Retries exhausted");
                    return Err(err.append(format!("On attempt {attempt}")));
                };
                self.report(RetryOutcome::Retried);
                (self.sleep_fn)(backoff.max(min_wait)).await;
            }
        }
    }
}
