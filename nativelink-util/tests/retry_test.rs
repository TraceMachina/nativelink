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
use core::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::ready;
use futures::stream::repeat_with;
use nativelink_config::stores::Retry;
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_util::retry::{Retrier, RetryOutcome, RetryResult};
use pretty_assertions::assert_eq;
use tokio::time::Duration;

#[nativelink_test]
async fn retry_simple_success() -> Result<(), Error> {
    let retrier = Retrier::new(
        Arc::new(|_duration| Box::pin(ready(()))),
        Arc::new(move |_delay| Duration::from_millis(1)),
        Retry {
            max_retries: 5,
            ..Default::default()
        },
    );
    let run_count = Arc::new(AtomicI32::new(0));

    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            run_count.fetch_add(1, Ordering::Relaxed);
            RetryResult::Ok(true)
        }))
        .await?;
    assert_eq!(
        run_count.load(Ordering::Relaxed),
        1,
        "Expected function to be called once"
    );
    assert_eq!(result, true, "Expected result to succeed");

    Ok(())
}

#[nativelink_test]
async fn retry_fails_after_3_runs() -> Result<(), Error> {
    let retrier = Retrier::new(
        Arc::new(|_duration| Box::pin(ready(()))),
        Arc::new(move |_delay| Duration::from_millis(1)),
        Retry {
            max_retries: 2,
            ..Default::default()
        },
    );
    let run_count = Arc::new(AtomicI32::new(0));
    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            run_count.fetch_add(1, Ordering::Relaxed);
            RetryResult::<bool>::Retry(make_err!(Code::Unavailable, "Dummy failure",))
        }))
        .await;
    assert_eq!(
        run_count.load(Ordering::Relaxed),
        3,
        "Expected function to be called"
    );
    assert_eq!(result.is_err(), true, "Expected result to error");
    assert_eq!(
        result.unwrap_err().to_string(),
        "Error { code: Unavailable, messages: [\"Dummy failure\", \"On attempt 3\"] }"
    );

    Ok(())
}

#[nativelink_test]
async fn dont_retry_for_not_found() -> Result<(), Error> {
    let retrier = Retrier::new(
        Arc::new(|_duration| Box::pin(ready(()))),
        Arc::new(move |_delay| Duration::from_millis(1)),
        Retry {
            max_retries: 2,
            ..Default::default()
        },
    );
    let run_count = Arc::new(AtomicI32::new(0));
    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            run_count.fetch_add(1, Ordering::Relaxed);
            RetryResult::<bool>::Retry(make_err!(Code::NotFound, "Dummy failure",))
        }))
        .await;
    assert_eq!(
        run_count.load(Ordering::Relaxed),
        1,
        "Expected function to be called once"
    );
    assert_eq!(result.is_err(), true, "Expected result to error");
    assert_eq!(
        result.unwrap_err().to_string(),
        "Error { code: NotFound, messages: [\"Dummy failure\"] }"
    );
    assert!(logs_contain("Not found, not retrying"));
    assert!(!logs_contain("ERROR"));

    Ok(())
}

#[nativelink_test]
async fn retry_success_after_2_runs() -> Result<(), Error> {
    let retrier = Retrier::new(
        Arc::new(|_duration| Box::pin(ready(()))),
        Arc::new(move |_delay| Duration::from_millis(1)),
        Retry {
            max_retries: 3,
            ..Default::default()
        },
    );
    let run_count = Arc::new(AtomicI32::new(0));

    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            run_count.fetch_add(1, Ordering::Relaxed);
            if run_count.load(Ordering::Relaxed) == 2 {
                return RetryResult::Ok(true);
            }
            RetryResult::<bool>::Retry(make_err!(Code::Unavailable, "Dummy failure",))
        }))
        .await?;
    assert_eq!(
        run_count.load(Ordering::Relaxed),
        2,
        "Expected function to be called"
    );
    assert_eq!(result, true, "Expected result to succeed");

    Ok(())
}

#[nativelink_test]
async fn retry_calls_sleep_fn() -> Result<(), Error> {
    const EXPECTED_MS: u64 = 71;
    let sleep_fn_run_count = Arc::new(AtomicI32::new(0));
    let sleep_fn_run_count_copy = sleep_fn_run_count.clone();
    let retrier = Retrier::new(
        Arc::new(move |duration| {
            // Note: Need to make another copy to make the compiler happy.
            let sleep_fn_run_count_copy = sleep_fn_run_count_copy.clone();
            Box::pin(async move {
                // Remember: This function is called only on retries, not the first run.
                sleep_fn_run_count_copy.fetch_add(1, Ordering::Relaxed);
                assert_eq!(duration, Duration::from_millis(EXPECTED_MS));
            })
        }),
        Arc::new(move |_delay| Duration::from_millis(EXPECTED_MS)),
        Retry {
            max_retries: 5,
            ..Default::default()
        },
    );

    {
        // Try with retry limit hit.
        let result = Pin::new(&retrier)
            .retry(repeat_with(|| {
                RetryResult::<bool>::Retry(make_err!(Code::Unavailable, "Dummy failure",))
            }))
            .await;

        assert_eq!(result.is_err(), true, "Expected the retry to fail");
        assert_eq!(
            sleep_fn_run_count.load(Ordering::Relaxed),
            5,
            "Expected the sleep_fn to be called twice"
        );
    }
    sleep_fn_run_count.store(0, Ordering::Relaxed); // Reset our counter.
    {
        // Try with 3 retries.
        let run_count = Arc::new(AtomicI32::new(0));
        let result = Pin::new(&retrier)
            .retry(repeat_with(|| {
                run_count.fetch_add(1, Ordering::Relaxed);
                // Remember: This function is only called every time, not just retries.
                // We run the first time, then retry 2 additional times meaning 3 runs.
                if run_count.load(Ordering::Relaxed) == 3 {
                    return RetryResult::Ok(true);
                }
                RetryResult::<bool>::Retry(make_err!(Code::Unavailable, "Dummy failure",))
            }))
            .await?;

        assert_eq!(result, true, "Expected results to pass");
        assert_eq!(
            sleep_fn_run_count.load(Ordering::Relaxed),
            2,
            "Expected the sleep_fn to be called twice"
        );
    }

    Ok(())
}

/// A retrier whose outcomes land in the returned list, with `max_retries`.
fn observed_retrier(max_retries: usize) -> (Retrier, Arc<Mutex<Vec<RetryOutcome>>>) {
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let sink = outcomes.clone();
    let retrier = Retrier::new(
        Arc::new(|_duration| Box::pin(ready(()))),
        Arc::new(move |_delay| Duration::from_millis(1)),
        Retry {
            max_retries,
            ..Default::default()
        },
    )
    .with_client("test")
    .with_outcome_fn(Arc::new(move |outcome| sink.lock().unwrap().push(outcome)));
    (retrier, outcomes)
}

/// Each retry is reported, and a success after them is a recovery; a success
/// on the first attempt reports nothing.
#[nativelink_test]
async fn retry_reports_retries_and_recovery() -> Result<(), Error> {
    let (retrier, outcomes) = observed_retrier(5);
    let run_count = AtomicI32::new(0);
    Pin::new(&retrier)
        .retry(repeat_with(|| {
            if run_count.fetch_add(1, Ordering::Relaxed) < 2 {
                RetryResult::Retry(make_err!(Code::Unavailable, "Dummy failure"))
            } else {
                RetryResult::Ok(true)
            }
        }))
        .await?;
    assert_eq!(
        *outcomes.lock().unwrap(),
        vec![
            RetryOutcome::Retried,
            RetryOutcome::Retried,
            RetryOutcome::Recovered
        ]
    );

    let (retrier, outcomes) = observed_retrier(5);
    Pin::new(&retrier)
        .retry(repeat_with(|| RetryResult::Ok(true)))
        .await?;
    assert_eq!(*outcomes.lock().unwrap(), vec![]);
    Ok(())
}

/// Retries that run out are reported as exhausted after the last retry.
#[nativelink_test]
async fn retry_reports_exhaustion() -> Result<(), Error> {
    let (retrier, outcomes) = observed_retrier(2);
    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            RetryResult::<bool>::Retry(make_err!(Code::Unavailable, "Dummy failure"))
        }))
        .await;
    assert!(result.is_err());
    assert_eq!(
        *outcomes.lock().unwrap(),
        vec![
            RetryOutcome::Retried,
            RetryOutcome::Retried,
            RetryOutcome::Exhausted
        ]
    );
    Ok(())
}

/// An error that is never retried is reported as permanent, except
/// `NotFound`, which on a store is an ordinary miss.
#[nativelink_test]
async fn retry_reports_permanent_errors_but_not_misses() -> Result<(), Error> {
    let (retrier, outcomes) = observed_retrier(5);
    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            RetryResult::<bool>::Retry(make_err!(Code::PermissionDenied, "Dummy failure"))
        }))
        .await;
    assert!(result.is_err());
    assert_eq!(*outcomes.lock().unwrap(), vec![RetryOutcome::Permanent]);

    let (retrier, outcomes) = observed_retrier(5);
    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            RetryResult::<bool>::Retry(make_err!(Code::NotFound, "Dummy miss"))
        }))
        .await;
    assert!(result.is_err());
    assert_eq!(*outcomes.lock().unwrap(), vec![]);
    Ok(())
}

fn recording_retrier(backoff: Duration, sleeps: &Arc<Mutex<Vec<Duration>>>) -> Retrier {
    let sleeps = Arc::clone(sleeps);
    Retrier::new(
        Arc::new(move |duration| {
            sleeps.lock().unwrap().push(duration);
            Box::pin(ready(()))
        }),
        Arc::new(move |_delay| backoff),
        Retry {
            max_retries: 3,
            ..Default::default()
        },
    )
}

#[nativelink_test]
async fn retry_after_hint_wins_when_longer_than_backoff() -> Result<(), Error> {
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let retrier = recording_retrier(Duration::from_millis(200), &sleeps);
    let run_count = Arc::new(AtomicI32::new(0));

    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            if run_count.fetch_add(1, Ordering::Relaxed) == 0 {
                return RetryResult::RetryAfter(
                    make_err!(Code::Unavailable, "Throttled"),
                    Duration::from_secs(3),
                );
            }
            RetryResult::Ok(true)
        }))
        .await?;

    assert_eq!(result, true);
    assert_eq!(run_count.load(Ordering::Relaxed), 2);
    assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_secs(3)]);
    Ok(())
}

#[nativelink_test]
async fn retry_after_backoff_wins_when_longer_than_hint() -> Result<(), Error> {
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let retrier = recording_retrier(Duration::from_secs(2), &sleeps);
    let run_count = Arc::new(AtomicI32::new(0));

    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            if run_count.fetch_add(1, Ordering::Relaxed) == 0 {
                return RetryResult::RetryAfter(
                    make_err!(Code::Unavailable, "Throttled"),
                    Duration::from_millis(50),
                );
            }
            RetryResult::Ok(true)
        }))
        .await?;

    assert_eq!(result, true);
    assert_eq!(run_count.load(Ordering::Relaxed), 2);
    assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_secs(2)]);
    Ok(())
}

/// With a throttle budget, a `RetryAfter` waits what it asked and spends
/// no attempt: three throttles ride out on one allowed retry.
#[nativelink_test]
async fn throttles_within_the_budget_spend_no_attempt() -> Result<(), Error> {
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let retrier = Retrier::new(
        Arc::new({
            let sleeps = Arc::clone(&sleeps);
            move |duration| {
                sleeps.lock().unwrap().push(duration);
                Box::pin(ready(()))
            }
        }),
        Arc::new(|_delay| Duration::from_millis(200)),
        Retry {
            max_retries: 1,
            ..Default::default()
        },
    )
    .with_throttle_budget(Duration::from_secs(10));
    let run_count = Arc::new(AtomicI32::new(0));

    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            if run_count.fetch_add(1, Ordering::Relaxed) < 3 {
                return RetryResult::RetryAfter(
                    make_err!(Code::Unavailable, "Throttled"),
                    Duration::from_secs(2),
                );
            }
            RetryResult::Ok(true)
        }))
        .await?;

    assert_eq!(result, true);
    assert_eq!(run_count.load(Ordering::Relaxed), 4);
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_secs(2); 3],
        "each throttle waits what it asked, not the backoff"
    );
    Ok(())
}

/// Once the budget is spent, a throttle spends an attempt like any other
/// transient error, so a throttle that never ends still fails.
#[nativelink_test]
async fn throttles_past_the_budget_spend_attempts() -> Result<(), Error> {
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let retrier = Retrier::new(
        Arc::new({
            let sleeps = Arc::clone(&sleeps);
            move |duration| {
                sleeps.lock().unwrap().push(duration);
                Box::pin(ready(()))
            }
        }),
        Arc::new(|_delay| Duration::from_millis(200)),
        Retry {
            max_retries: 1,
            ..Default::default()
        },
    )
    .with_throttle_budget(Duration::from_secs(3));
    let run_count = Arc::new(AtomicI32::new(0));

    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            run_count.fetch_add(1, Ordering::Relaxed);
            RetryResult::<bool>::RetryAfter(
                make_err!(Code::Unavailable, "Throttled"),
                Duration::from_secs(2),
            )
        }))
        .await;

    // Throttle 1 fits the budget (2 of 3 s). Throttle 2 would exceed it
    // and spends the one retry. Throttle 3 has no attempt left.
    assert_eq!(result.unwrap_err().code, Code::Unavailable);
    assert_eq!(run_count.load(Ordering::Relaxed), 3);
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![Duration::from_secs(2), Duration::from_secs(2)]
    );
    Ok(())
}

/// A throttle with no wait at all cannot spin inside the budget: it goes
/// through the backoff and spends an attempt.
#[nativelink_test]
async fn a_zero_wait_throttle_is_not_budgeted() -> Result<(), Error> {
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let retrier = recording_retrier(Duration::from_millis(200), &sleeps)
        .with_throttle_budget(Duration::from_secs(10));
    let run_count = Arc::new(AtomicI32::new(0));

    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            run_count.fetch_add(1, Ordering::Relaxed);
            RetryResult::<bool>::RetryAfter(
                make_err!(Code::Unavailable, "Throttled"),
                Duration::ZERO,
            )
        }))
        .await;

    assert_eq!(result.unwrap_err().code, Code::Unavailable);
    assert_eq!(
        run_count.load(Ordering::Relaxed),
        4,
        "three retries, then out"
    );
    assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_millis(200); 3]);
    Ok(())
}

#[nativelink_test]
async fn retry_after_with_permanent_code_returns_at_once() -> Result<(), Error> {
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let retrier = recording_retrier(Duration::from_millis(200), &sleeps);
    let run_count = Arc::new(AtomicI32::new(0));

    let result = Pin::new(&retrier)
        .retry(repeat_with(|| {
            run_count.fetch_add(1, Ordering::Relaxed);
            RetryResult::<bool>::RetryAfter(
                make_err!(Code::NotFound, "Dummy failure"),
                Duration::from_secs(5),
            )
        }))
        .await;

    assert_eq!(result.unwrap_err().code, Code::NotFound);
    assert_eq!(run_count.load(Ordering::Relaxed), 1);
    assert!(sleeps.lock().unwrap().is_empty());
    Ok(())
}

/// A throttle waited out within the budget spends no attempt but is still a
/// retry, and a success after only such throttles is a recovery.
#[nativelink_test]
async fn throttles_within_the_budget_report_retries_and_recovery() -> Result<(), Error> {
    let (retrier, outcomes) = observed_retrier(1);
    let retrier = retrier.with_throttle_budget(Duration::from_secs(10));
    let run_count = AtomicI32::new(0);
    Pin::new(&retrier)
        .retry(repeat_with(|| {
            if run_count.fetch_add(1, Ordering::Relaxed) < 2 {
                return RetryResult::RetryAfter(
                    make_err!(Code::Unavailable, "Throttled"),
                    Duration::from_secs(2),
                );
            }
            RetryResult::Ok(true)
        }))
        .await?;
    assert_eq!(
        *outcomes.lock().unwrap(),
        vec![
            RetryOutcome::Retried,
            RetryOutcome::Retried,
            RetryOutcome::Recovered
        ]
    );
    Ok(())
}
