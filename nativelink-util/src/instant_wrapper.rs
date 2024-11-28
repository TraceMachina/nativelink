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

use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mock_instant::thread_local::{Instant as MockInstant, MockClock};

/// Wrapper used to abstract away which underlying Instant impl we are using.
/// This is needed for testing.
pub trait InstantWrapper: Send + Sync + Unpin + 'static {
    fn from_secs(secs: u64) -> Self;
    fn unix_timestamp(&self) -> u64;
    fn now(&self) -> SystemTime;
    fn elapsed(&self) -> Duration;
    fn sleep(self, duration: Duration) -> impl Future<Output = ()> + Send + Sync + 'static;
}

impl InstantWrapper for SystemTime {
    fn from_secs(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(secs))
            .unwrap()
    }

    fn unix_timestamp(&self) -> u64 {
        self.duration_since(UNIX_EPOCH).unwrap().as_secs()
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn elapsed(&self) -> Duration {
        <SystemTime>::elapsed(self).unwrap()
    }

    async fn sleep(self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

pub fn default_instant_wrapper() -> impl InstantWrapper {
    SystemTime::now()
}

/// Our mocked out instant that we can pass to our `EvictionMap`.
pub struct MockInstantWrapped(MockInstant);

impl Default for MockInstantWrapped {
    fn default() -> Self {
        Self(MockInstant::now())
    }
}

impl InstantWrapper for MockInstantWrapped {
    fn from_secs(_secs: u64) -> Self {
        MockInstantWrapped(MockInstant::now())
    }

    fn unix_timestamp(&self) -> u64 {
        MockClock::time().as_secs()
    }

    fn now(&self) -> SystemTime {
        UNIX_EPOCH + MockClock::time()
    }

    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    async fn sleep(self, duration: Duration) {
        let baseline = self.0.elapsed();
        loop {
            tokio::task::yield_now().await;
            if self.0.elapsed() - baseline >= duration {
                break;
            }
        }
    }
}
