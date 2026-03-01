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

use async_trait::async_trait;
use nativelink_error::Error;
use nativelink_metric::RootMetricsComponent;

use crate::operation_state_manager::ClientStateManager;

/// `KnownPlatformPropertyProvider` interface is responsible for retrieving
/// a list of known platform properties.
// TODO(https://github.com/rust-lang/rust/issues/65991) When this lands we can
// move this to the nativelink-scheduler crate.
#[async_trait]
pub trait KnownPlatformPropertyProvider:
    ClientStateManager + Sync + Send + Unpin + RootMetricsComponent + 'static
{
    // Returns the platform property manager.
    async fn get_known_properties(&self, instance_name: &str) -> Result<Vec<String>, Error>;
}
