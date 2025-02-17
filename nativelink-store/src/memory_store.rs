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

use std::borrow::Borrow;
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::ops::Bound;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use alloc_bytes::{AllocBytesError, AllocBytesMut};
use async_trait::async_trait;
use bytes::Bytes;
use heap_allocator::MutexOwnedFixedHeapAllocator;
use nativelink_config::stores::MemorySpec;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::evicting_map::{EvictingMap, LenEntry};
use nativelink_util::health_utils::{
    default_health_status_indicator, HealthRegistryBuilder, HealthStatusIndicator,
};
use nativelink_util::store_trait::{StoreDriver, StoreKey, StoreKeyBorrow, UploadSizeInfo};
use tracing::{event, Level};

use crate::cas_utils::is_zero_digest;

/// Default amount of memory to evict when the cache is full.
const DEFAULT_EVICT_BYTES: usize = 2 * 1024 * 1024; // 4MB.

/// The maximum size of memory that can be allocated in a single call.
const MEMORY_ALLOC_CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MB.

#[derive(Clone)]
struct BytesWrapper(Bytes);

impl Debug for BytesWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

#[derive(MetricsComponent)]
pub struct MemoryStore {
    #[metric(group = "evicting_map")]
    evicting_map: EvictingMap<StoreKeyBorrow, BytesWrapper, SystemTime>,
    heap_allocator: Arc<MutexOwnedFixedHeapAllocator<Vec<MaybeUninit<u8>>>>,
}

impl MemoryStore {
    pub fn new(spec: &MemorySpec) -> Arc<Self> {
        let mut eviction_policy = spec.eviction_policy.clone().unwrap_or_default();
        if eviction_policy.evict_bytes == 0 {
            eviction_policy.evict_bytes = DEFAULT_EVICT_BYTES;
        }
        if eviction_policy.max_bytes == 0 {
            eviction_policy.max_bytes = DEFAULT_EVICT_BYTES * 10;
            event!(
                Level::WARN,
                "MemoryStore config' max_bytes is 0, setting to 10x evict_bytes({}). This will be required to be set in a future release.",
                eviction_policy.max_bytes,
            );
        }
        let max_bytes = eviction_policy.max_bytes;
        let mut data = Vec::<MaybeUninit<u8>>::with_capacity(max_bytes);
        // Safety: We are setting the length of the vector to the maximum
        unsafe { data.set_len(max_bytes) };
        let heap_allocator = Arc::new(MutexOwnedFixedHeapAllocator::new(data));
        Arc::new(Self {
            evicting_map: EvictingMap::new(&eviction_policy, SystemTime::now()),
            heap_allocator,
        })
    }

    /// Returns the number of key-value pairs currently in the cache.
    /// (Not used in production.)
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
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        // Internally Bytes might hold a reference to more data than just our data. To prevent
        // this potential case, we make a full copy of our data for long-term storage.
        let final_buffer = {
            let max_size = match size_info {
                UploadSizeInfo::ExactSize(size) | UploadSizeInfo::MaxSize(size) => {
                    if size > self.evicting_map.max_bytes() {
                        event!(
                            Level::WARN,
                            "Uploaded object too large {size} > {} in memory store, suggest wrapping in a size_partitioning store to ensure big files don't get sent to this store.",
                            self.evicting_map.max_bytes()
                        );
                        reader.drain().await.err_tip(|| "Failed to drain reader in memory_store::update")?;
                        return Ok(());
                    }
                    usize::try_from(size)
                }
            }
            .err_tip(|| "Could not convert size to usize in memory_store::update")?;

            let mut alloc_bytes_mut = AllocBytesMut::new_unaligned(self.heap_allocator.clone());
            loop {
                let chunk = reader
                    .recv()
                    .await
                    .err_tip(|| "Failed to receive data in memory_store::update")?;
                if chunk.is_empty() {
                    break; // EOF.
                }
                let new_size = alloc_bytes_mut.len().saturating_add(chunk.len());
                if new_size > alloc_bytes_mut.capacity() {
                    let additional_reserve = max_size
                        .checked_sub(alloc_bytes_mut.len())
                        .err_tip(|| "More bytes received than stated in memory store")?
                        .min(MEMORY_ALLOC_CHUNK_SIZE);
                    loop {
                        match alloc_bytes_mut.reserve(additional_reserve) {
                            Ok(()) => break,
                            Err(AllocBytesError::FailedToRealloc) => {
                                let evict_bytes = u64::try_from(chunk.len())
                                    .err_tip(|| "Could not convert chunk.len() to u64")?;
                                if self.evicting_map.force_eviction_bytes(evict_bytes).await == 0 {
                                    event!(
                                        Level::WARN,
                                        "FailedToRealloc: Object too large ({max_size}) to fit in memory store even after evicting. {}",
                                        "Suggest using a size_partitioning store to ensure big files don't get sent to this store.",
                                    );
                                    return Ok(());
                                }
                            }
                            Err(AllocBytesError::ReserveTooLarge) => {
                                event!(
                                    Level::WARN,
                                    "ReserveTooLarge: Object too large ({max_size}) to fit in memory store even after evicting. {}",
                                    "Suggest using a size_partitioning store to ensure big files don't get sent to this store.",
                                );
                                return Ok(());
                            }
                            Err(AllocBytesError::Layout(e)) => {
                                return Err(make_err!(
                                    Code::Internal,
                                    "Memory store Invalid layout. This should never happen - {e:?}"
                                ));
                            }
                        }
                    }
                }
                alloc_bytes_mut
                    .extend_from_slice(&chunk)
                    .map_err(|e| make_err!(
                        Code::Internal,
                        "Could not allocate bytes from slice in memory store. This should never happen - {e:?}")
                    )?;
            }
            Bytes::from_owner(alloc_bytes_mut.freeze())
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

    fn as_any<'a>(&'a self) -> &'a (dyn std::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }
}

default_health_status_indicator!(MemoryStore);
