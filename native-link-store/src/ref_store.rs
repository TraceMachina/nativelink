// Copyright 2022 The Native Link Authors. All rights reserved.
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

use std::cell::UnsafeCell;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use error::{make_err, make_input_err, Code, Error, ResultExt};
use native_link_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use native_link_util::common::DigestInfo;
use native_link_util::store_trait::{Store, UploadSizeInfo};

use crate::store_manager::StoreManager;

#[repr(C, align(8))]
struct AlignedStoreCell(UnsafeCell<Option<Arc<dyn Store>>>);

struct StoreReference {
    cell: AlignedStoreCell,
    mux: Mutex<()>,
}

unsafe impl Sync for StoreReference {}

pub struct RefStore {
    ref_store_name: String,
    store_manager: Weak<StoreManager>,
    ref_store: StoreReference,
}

impl RefStore {
    pub fn new(config: &native_link_config::stores::RefStore, store_manager: Weak<StoreManager>) -> Self {
        RefStore {
            ref_store_name: config.name.clone(),
            store_manager,
            ref_store: StoreReference {
                mux: Mutex::new(()),
                cell: AlignedStoreCell(UnsafeCell::new(None)),
            },
        }
    }

    // This will get the store or populate it if needed. It is designed to be quite fast on the
    // common path, but slow on the uncommon path. It does use some unsafe functions because we
    // wanted it to be fast. It is technically possible on some platforms for this function to
    // create a data race here is the reason I do not believe it is an issue:
    // 1. It would only happen on the very first call of the function (after first call we are safe)
    // 2. It should only happen on platforms that are < 64 bit address space
    // 3. It is likely that the internals of how Option work protect us anyway.
    #[inline]
    fn get_store(&self) -> Result<&Arc<dyn Store>, Error> {
        let ref_store = self.ref_store.cell.0.get();
        unsafe {
            if let Some(ref store) = *ref_store {
                return Ok(store);
            }
        }
        // This should protect us against multiple writers writing the same location at the same
        // time.
        let _lock = self
            .ref_store
            .mux
            .lock()
            .map_err(|e| make_err!(Code::Internal, "Failed to lock mutex in ref_store : {:?}", e))?;
        let store_manager = self.store_manager.upgrade().err_tip(|| "Store manager is gone")?;
        if let Some(store) = store_manager.get_store(&self.ref_store_name) {
            unsafe {
                *ref_store = Some(store.clone());
                return Ok((*ref_store).as_ref().unwrap());
            }
        }
        Err(make_input_err!(
            "Failed to find store '{}' in StoreManager in RefStore",
            self.ref_store_name
        ))
    }
}

#[async_trait]
impl Store for RefStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        let store = self.get_store()?;
        Pin::new(store.as_ref()).has_with_results(digests, results).await
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let store = self.get_store()?;
        Pin::new(store.as_ref()).update(digest, reader, size_info).await
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let store = self.get_store()?;
        Pin::new(store.as_ref())
            .get_part_ref(digest, writer, offset, length)
            .await
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
