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

use nativelink_config::stores::MemorySpec;
use nativelink_error::Code;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::store_trait::Store;
use pretty_assertions::assert_eq;

#[test]
fn add_store_rejects_duplicate_name() {
    let store_manager = StoreManager::new();
    store_manager
        .add_store("dup", Store::new(MemoryStore::new(&MemorySpec::default())))
        .expect("first add_store should succeed");
    let err = store_manager
        .add_store("dup", Store::new(MemoryStore::new(&MemorySpec::default())))
        .expect_err("duplicate store name must be rejected");
    assert_eq!(err.code, Code::AlreadyExists);
}
