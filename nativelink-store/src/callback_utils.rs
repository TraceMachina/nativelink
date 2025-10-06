// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use std::sync::Arc;

use nativelink_util::evicting_map::RemoveStateCallback;
use nativelink_util::store_trait::{RemoveItemCallback, StoreKey};
use tonic::async_trait;

// Generic struct to hold a RemoveItemCallback ref for the purposes
// of a RemoveStateCallback call
#[derive(Debug)]
pub struct RemoveItemCallbackHolder {
    callback_fn: Arc<Box<dyn RemoveItemCallback>>,
}

impl RemoveItemCallbackHolder {
    pub fn new(callback: &Arc<Box<dyn RemoveItemCallback>>) -> Self {
        Self {
            callback_fn: callback.clone(),
        }
    }
}

#[async_trait]
impl RemoveStateCallback<StoreKey<'static>> for RemoveItemCallbackHolder {
    async fn callback(&self, key: &StoreKey<'static>) {
        self.callback_fn.callback(key).await;
    }
}
