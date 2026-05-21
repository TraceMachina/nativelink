// Copyright 2024 Trace Machina, Inc. All rights reserved.
//
// Licensed under the Business Source License, Version 1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may requested a copy of the License by emailing contact@nativelink.com.
//
// Use of this module requires an enterprise license agreement, which can be
// attained by emailing contact@nativelink.com or signing up for Nativelink
// Cloud at app.nativelink.com.
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Bazel persistent worker protocol support for `NativeLink` workers.
//!
//! Public surface is intentionally small:
//! - [`WireFormat`] and the [`WorkRequest`]/[`WorkResponse`] types are the wire.
//! - [`PersistentWorkerPool`] is what `running_actions_manager` interacts with.
//! - [`WorkerKey`] is the equivalence class for worker reuse.
//!
//! See `docs/persistent-workers.md` for the design and §10 decisions that
//! shape this implementation.

pub mod live_worker;
pub mod pool;
pub mod protocol;

pub use live_worker::LiveWorker;
pub use pool::{
    DEFAULT_IDLE_TIMEOUT, DEFAULT_MAX_REQUESTS_PER_WORKER, DEFAULT_MAX_WORKERS_PER_KEY, Lease,
    PersistentWorkerPool, PoolConfig, WorkerKey,
};
pub use protocol::{Input, WireFormat, WorkRequest, WorkResponse};
