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
use bytes::Bytes;
use nativelink_config::stores::MemorySpec;
use nativelink_error::{Code, Error, ResultExt};
use tracing::{info, warn};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::evicting_map::{LenEntry, ShardedEvictingMap};
use nativelink_util::health_utils::{
    HealthRegistryBuilder, HealthStatusIndicator, default_health_status_indicator,
};
use nativelink_util::store_trait::{
    ItemCallback, StoreDriver, StoreKey, StoreKeyBorrow, StoreOptimizations, UploadSizeInfo,
};

use crate::callback_utils::ItemCallbackHolder;
use crate::cas_utils::is_zero_digest;

/// Scatter-gather buffer: stores data as a chain of `Bytes` chunks
/// (like BSD mbufs / Linux sk_buffs) to avoid concatenation copies.
/// Single-chunk and empty cases are common and handled without Vec overhead.
#[derive(Clone)]
pub struct BytesWrapper {
    /// Total byte length across all chunks.
    total_len: u64,
    /// The chunk chain. Single-element for oneshot writes, multi for streamed.
    chunks: Vec<Bytes>,
}

impl BytesWrapper {
    fn from_single(data: Bytes) -> Self {
        let total_len = data.len() as u64;
        if data.is_empty() {
            Self { total_len: 0, chunks: Vec::new() }
        } else {
            Self { total_len, chunks: vec![data] }
        }
    }

    fn from_chunks(chunks: Vec<Bytes>) -> Self {
        let total_len = chunks.iter().map(|c| c.len() as u64).sum();
        Self { total_len, chunks }
    }
}

impl Debug for BytesWrapper {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "BytesWrapper {{ len: {}, chunks: {} }}", self.total_len, self.chunks.len())
    }
}

impl LenEntry for BytesWrapper {
    #[inline]
    fn len(&self) -> u64 {
        self.total_len
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.total_len == 0
    }
}

#[derive(Debug, MetricsComponent)]
pub struct MemoryStore {
    #[metric(group = "evicting_map")]
    evicting_map: Arc<ShardedEvictingMap<
        StoreKeyBorrow,
        StoreKey<'static>,
        BytesWrapper,
        SystemTime,
        ItemCallbackHolder,
    >>,
}

impl MemoryStore {
    pub fn new(spec: &MemorySpec) -> Arc<Self> {
        let empty_policy = nativelink_config::stores::EvictionPolicy::default();
        let eviction_policy = spec.eviction_policy.as_ref().unwrap_or(&empty_policy);
        let evicting_map = Arc::new(ShardedEvictingMap::new(eviction_policy, SystemTime::now()));
        evicting_map.start_background_eviction();
        Arc::new(Self { evicting_map })
    }

    /// Returns the number of key-value pairs that are currently in the the cache.
    /// Function is not for production code paths.
    pub async fn len_for_test(&self) -> usize {
        self.evicting_map.len_for_test().await
    }

    pub async fn remove_entry(&self, key: StoreKey<'_>) -> bool {
        self.evicting_map.remove(&key.into_owned()).await
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
            .sizes_for_keys(
                keys.iter().map(|sk| sk.borrow().into_owned()),
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
        let update_start = std::time::Instant::now();
        info!(key = ?key, "MemoryStore::update: start");
        // Collect chunks without concatenation (scatter-gather).
        // Each chunk stays as its own Bytes allocation — no copies.
        let mut chunks = Vec::new();
        loop {
            let chunk = reader
                .recv()
                .await
                .err_tip(|| "Failed to recv in memory_store::update")?;
            if chunk.is_empty() {
                break; // EOF
            }
            chunks.push(chunk);
        }

        // Diagnostic: log if we received many tiny chunks for a non-tiny blob.
        // This would indicate the upstream is fragmenting unnecessarily.
        if chunks.len() > 2 {
            let total: usize = chunks.iter().map(|c| c.len()).sum();
            let avg = total / chunks.len();
            if avg < 4096 && total > 4096 {
                warn!(
                    key = ?key,
                    chunk_count = chunks.len(),
                    total_bytes = total,
                    avg_chunk_bytes = avg,
                    "memory_store::update: received many small chunks for non-small blob",
                );
            }
        }

        let owned_key = key.into_owned();
        let total_bytes: usize = chunks.iter().map(|c| c.len()).sum();
        self.evicting_map
            .insert(owned_key.clone().into(), BytesWrapper::from_chunks(chunks))
            .await;
        info!(
            key = ?owned_key,
            total_bytes,
            elapsed_ms = update_start.elapsed().as_millis() as u64,
            "MemoryStore::update: complete",
        );
        Ok(())
    }

    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        optimization == StoreOptimizations::SubscribesToUpdateOneshot
    }

    async fn update_oneshot(self: Pin<&Self>, key: StoreKey<'_>, data: Bytes) -> Result<(), Error> {
        let update_start = std::time::Instant::now();
        let data_len = data.len();
        info!(key = ?key, data_len, "MemoryStore::update_oneshot: start");
        // Small blobs may be slices of a much larger tonic receive buffer.
        // Copy them to avoid pinning the entire backing allocation in the
        // EvictingMap (e.g., 100-byte blob pinning a 16KiB h2 frame).
        // Large blobs are typically standalone allocations and safe to keep.
        let data = if !data.is_empty() && data.len() < 4096 {
            Bytes::copy_from_slice(&data)
        } else {
            data
        };
        let owned_key = key.into_owned();
        self.evicting_map
            .insert(owned_key.clone().into(), BytesWrapper::from_single(data))
            .await;
        info!(
            key = ?owned_key,
            data_len,
            elapsed_ms = update_start.elapsed().as_millis() as u64,
            "MemoryStore::update_oneshot: complete",
        );
        Ok(())
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let mut offset =
            usize::try_from(offset).err_tip(|| "Could not convert offset to usize")?;
        let length = length
            .map(|v| usize::try_from(v).err_tip(|| "Could not convert length to usize"))
            .transpose()?;

        let owned_key = key.into_owned();
        if is_zero_digest(owned_key.clone()) {
            writer
                .send_eof()
                .err_tip(|| "Failed to send zero EOF in memory store get_part")?;
            return Ok(());
        }

        let value = self
            .evicting_map
            .get(&owned_key)
            .await
            .err_tip_with_code(|_| (Code::NotFound, format!("Key {owned_key:?} not found")))?;
        let total_len = usize::try_from(value.len())
            .err_tip(|| "Could not convert value.len() to usize")?;
        let default_len = total_len.saturating_sub(offset);
        let mut remaining = length.unwrap_or(default_len).min(default_len);

        // Walk the chunk chain, sending each relevant piece without copying.
        for chunk in &value.chunks {
            if remaining == 0 {
                break;
            }
            let chunk_len = chunk.len();
            if offset >= chunk_len {
                // Skip this chunk entirely.
                offset -= chunk_len;
                continue;
            }
            let start = offset;
            let end = chunk_len.min(start + remaining);
            let slice = chunk.slice(start..end);
            remaining -= slice.len();
            offset = 0;
            writer
                .send(slice)
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

    fn register_item_callback(
        self: Arc<Self>,
        callback: Arc<dyn ItemCallback>,
    ) -> Result<(), Error> {
        self.evicting_map
            .add_item_callback(ItemCallbackHolder::new(callback));
        Ok(())
    }
}

default_health_status_indicator!(MemoryStore);
