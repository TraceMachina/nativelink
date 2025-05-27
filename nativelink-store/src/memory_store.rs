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
use nativelink_util::metrics::CACHE_TYPE;
use nativelink_util::store_trait::{StoreDriver, StoreKey, StoreKeyBorrow, UploadSizeInfo};
use opentelemetry::KeyValue;

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
    evicting_map: EvictingMap<StoreKeyBorrow, BytesWrapper, SystemTime>,
}

impl MemoryStore {
    pub fn new(spec: &MemorySpec) -> Arc<Self> {
        let empty_policy = nativelink_config::stores::EvictionPolicy::default();
        let eviction_policy = spec.eviction_policy.as_ref().unwrap_or(&empty_policy);
        Arc::new(Self {
            evicting_map: EvictingMap::new(
                eviction_policy,
                SystemTime::now(),
                &[KeyValue::new(CACHE_TYPE, "memory")],
            ),
        })
    }

    /// Returns the number of key-value pairs that are currently in the the cache.
    /// Function is not for production code paths.
    pub async fn len_for_test(&self) -> usize {
        self.evicting_map.len_for_test().await
    }

    pub async fn remove_entry(&self, key: StoreKey<'_>) -> bool {
        self.evicting_map.remove(&key).await
    }
}

#[async_trait]
impl StoreDriver for MemoryStore {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.evicting_map
            .sizes_for_keys::<_, StoreKey<'_>, &StoreKey<'_>>(
                keys.iter(),
                results,
                false, /* peek */
            )
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
            .range(range, move |key, _value| handler(key.borrow()))
            .await;
        Ok(iterations)
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
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

        if is_zero_digest(key.borrow()) {
            writer
                .send_eof()
                .err_tip(|| "Failed to send zero EOF in filesystem store get_part")?;
            return Ok(());
        }

        let value = self
            .evicting_map
            .get(&key)
            .await
            .err_tip_with_code(|_| (Code::NotFound, format!("Key {key:?} not found")))?;
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

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }
}

default_health_status_indicator!(MemoryStore);
