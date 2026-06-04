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

mod dynamic_fake_redis;
mod fake_redis;
mod pubsub;
mod read_only_redis;

pub use dynamic_fake_redis::{FakeRedisBackend, SubscriptionManagerNotify};
pub use fake_redis::{
    add_lua_script, add_to_response, add_to_response_raw, fake_redis_sentinel_master_stream,
    fake_redis_sentinel_stream, fake_redis_stream, make_fake_redis_with_responses,
};
pub use pubsub::MockPubSub;
pub use read_only_redis::ReadOnlyRedis;
