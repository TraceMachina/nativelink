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

use core::any::Any;
use core::borrow::Borrow;
use core::fmt::Debug;
use core::ops::Bound;
use core::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use nativelink_config::stores::MemorySpec;
use nativelink_error::{Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::evicting_map::{EvictingMap, LenEntry};
use nativelink_util::health_utils::{
    HealthRegistryBuilder, HealthStatusIndicator, default_health_status_indicator,
};
use nativelink_util::store_trait::{
    RemoveItemCallback, StoreDriver, StoreKey, StoreKeyBorrow, StoreOptimizations, UploadSizeInfo,
};

use crate::callback_utils::RemoveItemCallbackHolder;
use crate::cas_utils::is_zero_digest;

#[derive(Clone)]
pub struct BytesWrapper(Bytes);

impl Debug for BytesWrapper {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("BytesWrapper { -- Binary data -- }")
    }
}

impl LenEntry for BytesWrapper {
    #[inline]
    fn len(&self) -> u64 {
        Bytes::len(&self.0) as u64
    }

    #[inline]
    fn is_empty(&self) -> bool {
        Bytes::is_empty(&self.0)
    }
}

#[derive(Debug, MetricsComponent)]
pub struct MemoryStore {
    #[metric(group = "evicting_map")]
    evicting_map: EvictingMap<
        StoreKeyBorrow,
        StoreKey<'static>,
        BytesWrapper,
        SystemTime,
        RemoveItemCallbackHolder,
    >,
    /// The eviction policy's `max_bytes` (0 = unbounded). Cached here so `update`
    /// can skip writes larger than the entire store budget without buffering
    /// them — see the note in `update`.
    #[metric(help = "Maximum bytes this store will hold before eviction (0 = unbounded)")]
    max_bytes: u64,
}

impl MemoryStore {
    pub fn new(spec: &MemorySpec) -> Arc<Self> {
        let empty_policy = nativelink_config::stores::EvictionPolicy::default();
        let eviction_policy = spec.eviction_policy.as_ref().unwrap_or(&empty_policy);
        Arc::new(Self {
            max_bytes: eviction_policy.max_bytes as u64,
            evicting_map: EvictingMap::new(eviction_policy, SystemTime::now()),
        })
    }

    /// Returns the number of key-value pairs that are currently in the the cache.
    /// Function is not for production code paths.
    pub fn len_for_test(&self) -> usize {
        self.evicting_map.len_for_test()
    }

    pub async fn remove_entry(&self, key: StoreKey<'_>) -> bool {
        self.evicting_map.remove(&key.into_owned()).await
    }
}

#[async_trait]
impl StoreDriver for MemoryStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        let own_keys = keys
            .iter()
            .map(|sk| sk.borrow().into_owned())
            .collect::<Vec<_>>();
        self.evicting_map
            .sizes_for_keys(own_keys.iter(), results, false /* peek */)
            .await;
        // We need to do a special pass to ensure our zero digest exist.
        keys.iter()
            .zip(results.iter_mut())
            .for_each(|(key, result)| {
                if is_zero_digest(key.borrow()) {
                    *result = Some(0);
                }
            });
        Ok(())
    }

    async fn list(
        self: Pin<&Self>,
        range: (Bound<StoreKey<'_>>, Bound<StoreKey<'_>>),
        handler: &mut (dyn for<'a> FnMut(&'a StoreKey) -> bool + Send + Sync + '_),
    ) -> Result<u64, Error> {
        let range = (
            range.0.map(StoreKey::into_owned),
            range.1.map(StoreKey::into_owned),
        );
        let iterations = self
            .evicting_map
            .range(range, move |key, _value| handler(key.borrow()));
        Ok(iterations)
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        // A write whose exact size exceeds this store's `max_bytes` can never be
        // usefully cached: the moment it's inserted, eviction would drop it (and
        // everything else) since one entry alone is over the budget. Buffering it
        // into memory first is therefore pure waste — and under concurrent large
        // writes (e.g. a `memory` fast tier inside a `fast_slow` store fronting
        // CAS) it is a real OOM vector, because each in-flight write materializes
        // its whole payload before the eviction ever runs. Drain the stream and
        // skip instead. Only `ExactSize` is trusted here; a `MaxSize` upper bound
        // could over-estimate and wrongly skip a blob that would actually fit.
        if self.max_bytes != 0
            && let UploadSizeInfo::ExactSize(sz) = size_info
            && sz > self.max_bytes
        {
            let drained = reader
                .drain()
                .await
                .err_tip(|| "Failed to drain oversized write in memory_store::update")?;
            return Ok(drained);
        }
        // Internally Bytes might hold a reference to more data than just our data. To prevent
        // this potential case, we make a full copy of our data for long-term storage.
        let final_buffer = {
            let buffer = reader
                .consume(None)
                .await
                .err_tip(|| "Failed to collect all bytes from reader in memory_store::update")?;
            let mut new_buffer = BytesMut::with_capacity(buffer.len());
            new_buffer.extend_from_slice(&buffer[..]);
            new_buffer.freeze()
        };

        let len = final_buffer.len().try_into().unwrap_or(0);
        self.evicting_map
            .insert(key.into_owned().into(), BytesWrapper(final_buffer))
            .await;
        Ok(len)
    }

    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        optimization == StoreOptimizations::SubscribesToUpdateOneshot
    }

    async fn update_oneshot(self: Pin<&Self>, key: StoreKey<'_>, data: Bytes) -> Result<(), Error> {
        // Fast path: Direct insertion without channel overhead.
        // We still need to copy the data to prevent holding references to larger buffers.
        let final_buffer = if data.is_empty() {
            data
        } else {
            let mut new_buffer = BytesMut::with_capacity(data.len());
            new_buffer.extend_from_slice(&data[..]);
            new_buffer.freeze()
        };

        self.evicting_map
            .insert(key.into_owned().into(), BytesWrapper(final_buffer))
            .await;
        Ok(())
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let offset = usize::try_from(offset).err_tip(|| "Could not convert offset to usize")?;
        let length = length
            .map(|v| usize::try_from(v).err_tip(|| "Could not convert length to usize"))
            .transpose()?;

        let owned_key = key.into_owned();
        if is_zero_digest(owned_key.clone()) {
            writer
                .send_eof()
                .err_tip(|| "Failed to send zero EOF in filesystem store get_part")?;
            return Ok(());
        }

        let value = self
            .evicting_map
            .get(&owned_key)
            .await
            .err_tip_with_code(|_| (Code::NotFound, format!("Key {owned_key:?} not found")))?;
        let default_len = usize::try_from(value.len())
            .err_tip(|| "Could not convert value.len() to usize")?
            .saturating_sub(offset);
        let length = length.unwrap_or(default_len).min(default_len);
        if length > 0 {
            writer
                .send(value.0.slice(offset..(offset + length)))
                .await
                .err_tip(|| "Failed to write data in memory store")?;
        }
        writer
            .send_eof()
            .err_tip(|| "Failed to write EOF in memory store get_part")?;
        Ok(())
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn Any + Sync + Send + 'static> {
        self
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }

    fn register_remove_callback(
        self: Arc<Self>,
        callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        self.evicting_map
            .add_remove_callback(RemoveItemCallbackHolder::new(callback));
        Ok(())
    }
}

default_health_status_indicator!(MemoryStore);
