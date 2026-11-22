// Copyright 2026 Trace Machina, Inc. All rights reserved.
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

//! The fake Redis expires keys on tokio's clock, so a test on a paused
//! runtime sees a key with a TTL go when its time passes, as it would on
//! Redis.

use core::time::Duration;
use std::collections::HashMap;

use nativelink_macro::nativelink_test;
use nativelink_redis_tester::{FakeRedisBackend, SubscriptionManagerNotify};

struct NoSubscriptions;

impl SubscriptionManagerNotify for NoSubscriptions {
    fn notify_for_test(&self, _value: String) {}
}

fn has(fake: &FakeRedisBackend<NoSubscriptions>, key: &str) -> bool {
    fake.table.lock().unwrap().contains_key(key)
}

#[nativelink_test(start_paused = true)]
async fn a_key_goes_when_its_ttl_runs_out() {
    let fake = FakeRedisBackend::<NoSubscriptions>::new();
    fake.table
        .lock()
        .unwrap()
        .insert("ck_a".to_string(), HashMap::new());
    fake.set_ttl("ck_a", 30);
    assert_eq!(fake.expiries.lock().unwrap().get("ck_a"), Some(&30));

    tokio::time::advance(Duration::from_secs(29)).await;
    fake.remove_expired();
    assert!(has(&fake, "ck_a"), "a second of its TTL is left");
    assert_eq!(fake.time_to_live("ck_a"), Some(Duration::from_secs(1)));

    tokio::time::advance(Duration::from_secs(1)).await;
    fake.remove_expired();
    assert!(!has(&fake, "ck_a"), "gone once its TTL has run out");
    assert_eq!(fake.time_to_live("ck_a"), None);
    assert_eq!(fake.expiries.lock().unwrap().get("ck_a"), None);
}

#[nativelink_test(start_paused = true)]
async fn a_refreshed_ttl_starts_again_and_zero_removes_the_key() {
    let fake = FakeRedisBackend::<NoSubscriptions>::new();
    for key in ["ck_a", "ck_b"] {
        fake.table
            .lock()
            .unwrap()
            .insert(key.to_string(), HashMap::new());
    }
    fake.set_ttl("ck_a", 30);
    tokio::time::advance(Duration::from_secs(20)).await;
    fake.set_ttl("ck_a", 30);
    tokio::time::advance(Duration::from_secs(20)).await;
    fake.remove_expired();
    assert!(has(&fake, "ck_a"), "the refresh moved its deadline");

    fake.set_ttl("ck_b", 0);
    assert!(!has(&fake, "ck_b"), "a TTL that is not positive removes it");
}
