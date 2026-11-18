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

use std::sync::Arc;

use nativelink_config::stores::{ExternalSpec, MemorySpec, StoreSpec};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::external_store::{
    ExternalStoreFuture, register_external_store_provider, registered_external_store_providers,
};
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::{Store, StoreLike};
use pretty_assertions::assert_eq;

const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

/// A provider that builds a memory store from its `config` value, which
/// must be a (possibly empty) `MemorySpec`.
fn memory_backed_provider<'a>(
    spec: &'a ExternalSpec,
    _store_manager: &'a Arc<StoreManager>,
) -> ExternalStoreFuture<'a> {
    Box::pin(async move {
        let memory_spec: MemorySpec = serde_json::from_value(spec.config.clone())
            .map_err(|e| make_err!(Code::InvalidArgument, "Invalid test config: {e}"))?;
        Ok(Store::new(MemoryStore::new(&memory_spec)))
    })
}

#[nativelink_test]
async fn registered_provider_builds_store_through_factory() -> Result<(), Error> {
    const VALUE: &str = "external store data";
    register_external_store_provider("test_memory_roundtrip", Arc::new(memory_backed_provider))?;
    assert!(registered_external_store_providers().contains(&"test_memory_roundtrip".to_string()));

    let spec = StoreSpec::External(ExternalSpec {
        provider: "test_memory_roundtrip".to_string(),
        config: serde_json::json!({}),
    });
    let store_manager = Arc::new(StoreManager::new());
    let store = store_factory(&spec, &store_manager, None)
        .await
        .err_tip(|| "store_factory must build a registered external store")?;

    let digest = DigestInfo::try_new(VALID_HASH1, VALUE.len() as u64)?;
    store.update_oneshot(digest, VALUE.into()).await?;
    let data = store
        .get_part_unchunked(digest, 0, None)
        .await
        .err_tip(|| "reading back through the external store")?;
    assert_eq!(data, VALUE.as_bytes());
    Ok(())
}

#[nativelink_test]
async fn unknown_provider_fails_with_registered_list() -> Result<(), Error> {
    let spec = StoreSpec::External(ExternalSpec {
        provider: "test_provider_nobody_registered".to_string(),
        config: serde_json::json!({}),
    });
    let store_manager = Arc::new(StoreManager::new());
    let err = store_factory(&spec, &store_manager, None)
        .await
        .expect_err("an unregistered provider must fail to build");
    assert_eq!(err.code, Code::InvalidArgument);
    let message = format!("{err:?}");
    assert!(
        message.contains("'test_provider_nobody_registered' is not registered"),
        "error must name the unknown provider: {message}"
    );
    assert!(
        message.contains("Registered providers: ["),
        "error must list the registered providers: {message}"
    );
    Ok(())
}

#[nativelink_test]
async fn duplicate_provider_registration_is_rejected() -> Result<(), Error> {
    register_external_store_provider("test_duplicate_provider", Arc::new(memory_backed_provider))?;
    let err = register_external_store_provider(
        "test_duplicate_provider",
        Arc::new(memory_backed_provider),
    )
    .expect_err("registering the same provider twice must fail");
    assert_eq!(err.code, Code::AlreadyExists);
    Ok(())
}
