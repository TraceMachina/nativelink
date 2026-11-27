// Copyright 2024 Trace Machina, Inc. All rights reserved.
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

use core::fmt::Debug;
use core::future::Future;
use core::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use mock_instant::thread_local::MockClock;

/// Wrapper used to abstract away which underlying Instant impl we are using.
/// This is needed for testing.
pub trait InstantWrapper: Send + Sync + Unpin + Debug + 'static {
    fn from_secs(secs: u64) -> Self;
    fn unix_timestamp(&self) -> u64;
    fn now(&self) -> SystemTime;
    fn elapsed(&self) -> Duration;
    fn sleep(self, duration: Duration) -> impl Future<Output = ()> + Send + Sync + 'static;
}

impl InstantWrapper for SystemTime {
    fn from_secs(secs: u64) -> Self {
        Self::UNIX_EPOCH
            .checked_add(Duration::from_secs(secs))
            .unwrap()
    }

    fn unix_timestamp(&self) -> u64 {
        self.duration_since(UNIX_EPOCH).unwrap().as_secs()
    }

    fn now(&self) -> SystemTime {
        Self::now()
    }

    fn elapsed(&self) -> Duration {
        <Self>::elapsed(self).unwrap()
    }

    async fn sleep(self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

pub fn default_instant_wrapper() -> impl InstantWrapper {
    SystemTime::now()
}

/// Our mocked out instant that we can pass to our `EvictionMap`: a point on
/// the mock clock, kept as `MockClock::time()` (time since the Unix epoch in
/// tests that set it). Like `SystemTime`'s, `from_secs` is that second and
/// `unix_timestamp` is the instant's own, not the clock's current time, so
/// code that stores a timestamp and judges its age later behaves in a test
/// as it does in production.
#[derive(Debug, Clone, Copy)]
pub struct MockInstantWrapped(Duration);

impl Default for MockInstantWrapped {
    fn default() -> Self {
        Self(MockClock::time())
    }
}

impl InstantWrapper for MockInstantWrapped {
    fn from_secs(secs: u64) -> Self {
        Self(Duration::from_secs(secs))
    }

    fn unix_timestamp(&self) -> u64 {
        self.0.as_secs()
    }

    fn now(&self) -> SystemTime {
        UNIX_EPOCH + MockClock::time()
    }

    fn elapsed(&self) -> Duration {
        MockClock::time().saturating_sub(self.0)
    }

    async fn sleep(self, duration: Duration) {
        let baseline = self.elapsed();
        loop {
            tokio::task::yield_now().await;
            if self.elapsed().saturating_sub(baseline) >= duration {
                break;
            }
        }
    }
}
