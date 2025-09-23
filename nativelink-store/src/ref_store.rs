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

use core::cell::UnsafeCell;
use core::pin::Pin;
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use nativelink_config::stores::RefSpec;
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo};
use tracing::error;

use crate::store_manager::StoreManager;

#[repr(C, align(8))]
#[derive(Debug)]
struct AlignedStoreCell(UnsafeCell<Option<Store>>);

#[derive(Debug)]
struct StoreReference {
    cell: AlignedStoreCell,
    mux: Mutex<()>,
}

unsafe impl Sync for StoreReference {}

#[derive(Debug, MetricsComponent)]
pub struct RefStore {
    #[metric(help = "The store we are referencing")]
    name: String,
    store_manager: Weak<StoreManager>,
    inner: StoreReference,
}

impl RefStore {
    pub fn new(spec: &RefSpec, store_manager: Weak<StoreManager>) -> Arc<Self> {
        Arc::new(Self {
            name: spec.name.clone(),
            store_manager,
            inner: StoreReference {
                mux: Mutex::new(()),
                cell: AlignedStoreCell(UnsafeCell::new(None)),
            },
        })
    }

    // This will get the store or populate it if needed. It is designed to be quite fast on the
    // common path, but slow on the uncommon path. It does use some unsafe functions because we
    // wanted it to be fast. It is technically possible on some platforms for this function to
    // create a data race here is the reason I do not believe it is an issue:
    // 1. It would only happen on the very first call of the function (after first call we are safe)
    // 2. It should only happen on platforms that are < 64 bit address space
    // 3. It is likely that the internals of how Option work protect us anyway.
    #[inline]
    fn get_store(&self) -> Result<&Store, Error> {
        let ref_store = self.inner.cell.0.get();
        unsafe {
            if let Some(ref store) = *ref_store {
                return Ok(store);
            }
        }
        // This should protect us against multiple writers writing the same location at the same
        // time.
        let _lock = self.inner.mux.lock().map_err(|e| {
            make_err!(
                Code::Internal,
                "Failed to lock mutex in ref_store : {:?}",
                e
            )
        })?;
        let store_manager = self
            .store_manager
            .upgrade()
            .err_tip(|| "Store manager is gone")?;
        if let Some(store) = store_manager.get_store(&self.name) {
            unsafe {
                *ref_store = Some(store);
                return Ok((*ref_store).as_ref().unwrap());
            }
        }
        Err(make_input_err!(
            "Failed to find store '{}' in StoreManager in RefStore",
            self.name
        ))
    }
}

#[async_trait]
impl StoreDriver for RefStore {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.get_store()?.has_with_results(keys, results).await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        self.get_store()?.update(key, reader, size_info).await
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        self.get_store()?
            .get_part(key, writer, offset, length)
            .await
    }

    fn inner_store(&self, key: Option<StoreKey>) -> &'_ dyn StoreDriver {
        match self.get_store() {
            Ok(store) => store.inner_store(key),
            Err(err) => {
                error!(?key, ?err, "Failed to get store for key",);
                self
            }
        }
    }

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }
}

default_health_status_indicator!(RefStore);
