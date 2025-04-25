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

use async_trait::async_trait;
use nativelink_error::Error;

use crate::operation_state_manager::ClientStateManager;

/// KnownPlatformPropertyProvider interface is responsible for retrieving
/// a list of known platform properties.
// TODO(https://github.com/rust-lang/rust/issues/65991) When this lands we can
// move this to the nativelink-scheduler crate.
#[async_trait]
pub trait KnownPlatformPropertyProvider:
    ClientStateManager + Sync + Send + Unpin + 'static
{
    // Returns the platform property manager.
    async fn get_known_properties(&self, instance_name: &str) -> Result<Vec<String>, Error>;
}
