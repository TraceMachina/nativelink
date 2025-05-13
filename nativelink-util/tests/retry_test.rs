// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::pin::Pin;
use core::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use futures::future::ready;
use futures::stream::repeat_with;
use nativelink_config::stores::Retry;
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_util::retry::{Retrier, RetryResult};
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
