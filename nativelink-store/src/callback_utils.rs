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

use core::borrow::Borrow;
use core::pin::Pin;
use std::sync::Arc;

use nativelink_util::evicting_map;
use nativelink_util::store_trait::{RemoveItemCallback, StoreKey};

// Generic struct to hold a RemoveItemCallback ref for the purposes
// of a RemoveStateCallback call
#[derive(Debug)]
pub struct RemoveItemCallbackHolder {
    callback: Arc<dyn RemoveItemCallback>,
}

impl RemoveItemCallbackHolder {
    pub fn new(callback: Arc<dyn RemoveItemCallback>) -> Self {
        Self { callback }
    }
}

impl<'a, Q> evicting_map::RemoveItemCallback<Q> for RemoveItemCallbackHolder
where
    Q: Borrow<StoreKey<'a>>,
{
    fn callback(&self, store_key: &Q) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let callback = self.callback.clone();
        let store_key: &StoreKey<'_> = Borrow::<StoreKey<'_>>::borrow(store_key);
        let store_key = store_key.borrow().into_owned();
        Box::pin(async move { callback.callback(store_key).await })
    }
}
