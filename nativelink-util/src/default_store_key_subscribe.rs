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

use std::pin::Pin;
use std::sync::Arc;

use nativelink_error::{Error, ResultExt};
use tokio::sync::watch;
use tonic::async_trait;

use crate::background_spawn;
use crate::buf_channel::{make_buf_channel_pair, DropCloserWriteHalf};
use crate::common::DigestInfo;
use crate::digest_hasher::{DigestHasher, DigestHasherFunc};
use crate::store_trait::{Store, StoreSubscriptionItem};

struct DefaultStoreSubscriptionItem<S: Store + ?Sized> {
    store: Arc<S>,
    key: DigestInfo,
}

#[async_trait]
impl<S: Store + ?Sized> StoreSubscriptionItem for DefaultStoreSubscriptionItem<S> {
    async fn get_key(&self) -> Result<DigestInfo, Error> {
        Ok(self.key)
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        Pin::new(self.store.as_ref())
            .get_part_ref(self.key, writer, offset, length)
            .await
    }
}

/// Fetches all the data, hashes it and returns the hash of the data.
async fn get_data_version_key<S: Store + ?Sized>(
    store: Pin<&S>,
    key: DigestInfo,
) -> Result<DigestInfo, Error> {
    let mut hasher = DigestHasherFunc::Blake3.hasher();
    let (writer, reader) = make_buf_channel_pair();
    let read_fut = store.get_part(key, writer, 0, None);
    let (read_res, write_res) = futures::join!(read_fut, async move {
        let mut reader = reader;
        loop {
            let data = reader
                .recv()
                .await
                .err_tip(|| "Failed to receive data in get_data_version_key")?;
            hasher.update(&data);
            if data.is_empty() {
                break;
            }
        }
        Result::<_, Error>::Ok(hasher.finalize_digest())
    });
    if read_res.is_err() {
        return read_res.merge(write_res);
    }
    write_res
}

pub(crate) async fn default_store_key_subscribe<S: Store + ?Sized>(
    store: Arc<S>,
    key: DigestInfo,
) -> watch::Receiver<Result<Arc<dyn StoreSubscriptionItem>, Error>> {
    let last_version_key = match get_data_version_key(Pin::new(store.as_ref()), key).await {
        Ok(version_key) => version_key,
        Err(e) => {
            let (_, watch_rx) = watch::channel(Err(e));
            return watch_rx;
        }
    };

    let (watch_tx, watch_rx) = watch::channel::<Result<Arc<dyn StoreSubscriptionItem>, Error>>(Ok(
        Arc::new(DefaultStoreSubscriptionItem {
            store: store.clone(),
            key,
        }),
    ));

    background_spawn!("default_store_subscribe_spawn", async move {
        let mut last_version_key = last_version_key;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
                    match get_data_version_key(Pin::new(store.as_ref()), key).await {
                        Ok(next_version_key) => {
                            if next_version_key != last_version_key {
                                last_version_key = next_version_key;
                                watch_tx.send_modify(|_| {});
                            }
                        },
                        Err(e) => {
                            let _ = watch_tx.send(Err(e)); // If error, it means receiver has been dropped.
                            return;
                        },
                    }

                },
                _ = watch_tx.closed() => return, // The receiver has been dropped.
            }
        }
    });
    watch_rx
}
