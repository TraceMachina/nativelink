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

//! Registry for store providers supplied by the binary that embeds
//! `NativeLink`. The stock `nativelink` binary registers no providers;
//! a distribution that links extra store implementations registers
//! each one before the server starts, and configs select them with
//! the `external` store spec.

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use futures::Future;
use nativelink_config::stores::ExternalSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_util::store_trait::Store;
use parking_lot::RwLock;
use tracing::info;

use crate::store_manager::StoreManager;

/// Future returned by an external store provider while it constructs
/// its store.
pub type ExternalStoreFuture<'a> = Pin<Box<dyn Future<Output = Result<Store, Error>> + Send + 'a>>;

/// Constructor for an externally provided store. It receives the
/// `external` spec (whose `config` value is provider-defined) and the
/// store manager, so composed providers can build inner stores through
/// [`crate::default_store_factory::store_factory`].
pub type ExternalStoreFactory = Arc<
    dyn for<'a> Fn(&'a ExternalSpec, &'a Arc<StoreManager>) -> ExternalStoreFuture<'a>
        + Send
        + Sync,
>;

static EXTERNAL_STORE_REGISTRY: LazyLock<RwLock<HashMap<String, ExternalStoreFactory>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Registers `factory` as the constructor for `external` store specs
/// whose `provider` equals `provider`. Call before the server loads its
/// configuration. Registering the same provider twice is an error so a
/// misassembled binary fails loudly instead of silently picking one.
pub fn register_external_store_provider(
    provider: &str,
    factory: ExternalStoreFactory,
) -> Result<(), Error> {
    let mut registry = EXTERNAL_STORE_REGISTRY.write();
    if registry.contains_key(provider) {
        return Err(make_err!(
            Code::AlreadyExists,
            "External store provider '{provider}' is already registered"
        ));
    }
    registry.insert(provider.to_string(), factory);
    Ok(())
}

/// Names of every registered external store provider, sorted. Logged at
/// startup and echoed in unknown-provider errors so a config/binary
/// mismatch is diagnosable from the error alone.
pub fn registered_external_store_providers() -> Vec<String> {
    let mut providers: Vec<String> = EXTERNAL_STORE_REGISTRY.read().keys().cloned().collect();
    providers.sort();
    providers
}

/// Constructs the store an `external` spec names, through its registered
/// provider.
pub(crate) async fn make_external_store(
    spec: &ExternalSpec,
    store_manager: &Arc<StoreManager>,
) -> Result<Store, Error> {
    let maybe_factory = EXTERNAL_STORE_REGISTRY.read().get(&spec.provider).cloned();
    let Some(factory) = maybe_factory else {
        return Err(make_err!(
            Code::InvalidArgument,
            "External store provider '{}' is not registered in this binary. \
             Registered providers: [{}]. The stock open source nativelink \
             binary registers none; an 'external' store spec requires a \
             distribution that provides the named store implementation.",
            spec.provider,
            registered_external_store_providers().join(", "),
        ));
    };
    info!(
        target: "nativelink_store::external",
        provider = %spec.provider,
        "Constructing external store through its registered provider"
    );
    factory(spec, store_manager)
        .await
        .err_tip(|| format!("In external store provider '{}'", spec.provider))
}
