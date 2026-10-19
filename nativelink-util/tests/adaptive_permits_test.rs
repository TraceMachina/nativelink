// Copyright 2026 The NativeLink Authors. All rights reserved.
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

use core::time::Duration;

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_util::adaptive_permits::AdaptivePermits;
use pretty_assertions::assert_eq;

/// A throttle halves the limit; the floor holds.
#[nativelink_test(start_paused = true)]
async fn a_throttle_halves_the_limit_down_to_the_floor() -> Result<(), Error> {
    let pool = AdaptivePermits::new("test", 64, 4, 256, 10);
    assert_eq!(pool.limit(), 64);
    pool.record_throttle();
    assert_eq!(pool.limit(), 32);
    assert_eq!(pool.available(), 32, "idle permits are taken back at once");
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        pool.record_throttle();
    }
    assert_eq!(pool.limit(), 4, "never below the floor");
    assert_eq!(pool.available(), 4);
    Ok(())
}

/// Two throttles within one round trip are one event.
#[nativelink_test]
async fn throttles_in_the_same_window_cut_once() -> Result<(), Error> {
    let pool = AdaptivePermits::new("test", 64, 4, 256, 10);
    pool.record_throttle();
    pool.record_throttle();
    pool.record_throttle();
    assert_eq!(pool.limit(), 32);
    Ok(())
}

/// A run of successes adds one permit; the ceiling holds.
#[nativelink_test]
async fn a_streak_of_successes_grows_the_limit_to_the_ceiling() -> Result<(), Error> {
    let pool = AdaptivePermits::new("test", 8, 4, 10, 3);
    for _ in 0..2 {
        pool.record_success();
    }
    assert_eq!(pool.limit(), 8, "two of three is not a streak");
    pool.record_success();
    assert_eq!(pool.limit(), 9);
    assert_eq!(pool.available(), 9);
    for _ in 0..30 {
        pool.record_success();
    }
    assert_eq!(pool.limit(), 10, "never above the ceiling");
    assert_eq!(pool.available(), 10);
    Ok(())
}

/// A throttle resets the streak, so growth starts over from the cut.
#[nativelink_test]
async fn a_throttle_resets_the_streak() -> Result<(), Error> {
    let pool = AdaptivePermits::new("test", 8, 4, 16, 3);
    pool.record_success();
    pool.record_success();
    pool.record_throttle();
    assert_eq!(pool.limit(), 4);
    pool.record_success();
    assert_eq!(pool.limit(), 4, "the two before the cut do not count");
    pool.record_success();
    pool.record_success();
    assert_eq!(pool.limit(), 5);
    Ok(())
}

/// A cut while permits are out takes back what is idle and forgets the
/// rest as they are released, so the pool never exceeds the new limit.
#[nativelink_test]
async fn a_cut_with_permits_out_is_settled_on_release() -> Result<(), Error> {
    let pool = AdaptivePermits::new("test", 8, 2, 8, 0);
    let held: Vec<_> = {
        let mut held = Vec::new();
        for _ in 0..6 {
            held.push(pool.acquire().await);
        }
        held
    };
    assert_eq!(pool.available(), 2);
    pool.record_throttle();
    assert_eq!(pool.limit(), 4);
    assert_eq!(pool.available(), 0, "the two idle permits were taken back");
    drop(held);
    assert_eq!(
        pool.available(),
        4,
        "two of the six released permits were forgotten, four returned"
    );
    Ok(())
}

/// Growth while permits are still owed cancels an owed permit instead of
/// adding one, so the pool size tracks the limit exactly.
#[nativelink_test]
async fn growth_while_owed_cancels_the_debt_first() -> Result<(), Error> {
    let pool = AdaptivePermits::new("test", 8, 2, 8, 1);
    let held: Vec<_> = {
        let mut held = Vec::new();
        for _ in 0..8 {
            held.push(pool.acquire().await);
        }
        held
    };
    pool.record_throttle();
    assert_eq!(pool.limit(), 4, "all four cut permits are owed");
    pool.record_success();
    assert_eq!(pool.limit(), 5);
    assert_eq!(pool.available(), 0, "the growth cancelled one owed permit");
    drop(held);
    assert_eq!(pool.available(), 5, "three forgotten, five returned");
    Ok(())
}

/// With `grow_after` zero the limit only ever falls.
#[nativelink_test]
async fn a_fixed_pool_never_grows() -> Result<(), Error> {
    let pool = AdaptivePermits::new("test", 8, 4, 8, 0);
    for _ in 0..100 {
        pool.record_success();
    }
    assert_eq!(pool.limit(), 8);
    pool.record_throttle();
    assert_eq!(pool.limit(), 4);
    for _ in 0..100 {
        pool.record_success();
    }
    assert_eq!(pool.limit(), 4);
    Ok(())
}

/// Acquire waits for a release when the pool is empty.
#[nativelink_test]
async fn acquire_waits_for_a_free_permit() -> Result<(), Error> {
    let pool = AdaptivePermits::new("test", 1, 1, 1, 0);
    let first = pool.acquire().await;
    let waiter = tokio::spawn({
        let pool = pool.clone();
        async move {
            let _second = pool.acquire().await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiter.is_finished(), "no permit to take yet");
    drop(first);
    tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("the waiter got the released permit")
        .expect("the waiter task did not panic");
    Ok(())
}
