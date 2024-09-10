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
use std::time::Duration;

use async_trait::async_trait;
use futures::Future;
use nativelink_error::{make_err, Code, Error, ResultExt};
use tokio::sync::watch;
use tokio::time::sleep;

use crate::background_spawn;
use crate::buf_channel::{make_buf_channel_pair, DropCloserWriteHalf};
use crate::common::DigestInfo;
use crate::digest_hasher::{DigestHasher, DigestHasherFunc};
use crate::store_trait::{StoreDriver, StoreKey, StoreSubscription, StoreSubscriptionItem};

/// The default sleep time for checking changes in store keys.
const DEFAULT_SLEEP_FOR_CHANGES: Duration = Duration::from_secs(10);

struct DefaultStoreSubscription(watch::Receiver<Result<Arc<dyn StoreSubscriptionItem>, Error>>);

#[async_trait]
impl StoreSubscription for DefaultStoreSubscription {
    fn peek(&self) -> Result<Arc<dyn StoreSubscriptionItem>, Error> {
        self.0.borrow().clone()
    }

    async fn changed(&mut self) -> Result<Arc<dyn StoreSubscriptionItem>, Error> {
        self.0.changed().await.map_err(|e| {
            make_err!(
                Code::Internal,
                "Sender dropped in DefaultStoreSubscription::changed - {e:?}"
            )
        })?;
        self.0.borrow().clone()
    }
}

struct DefaultStoreSubscriptionItem<S: StoreDriver + ?Sized> {
    store: Arc<S>,
    key: StoreKey<'static>,
}

#[async_trait]
impl<S: StoreDriver + ?Sized> StoreSubscriptionItem for DefaultStoreSubscriptionItem<S> {
    async fn get_key(&self) -> Result<StoreKey, Error> {
        Ok(self.key.borrow())
    }

    async fn get_part(
        &self,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        Pin::new(self.store.as_ref())
            .get_part(self.key.borrow(), writer, offset, length)
            .await
    }
}

/// Fetches all the data, hashes it and returns the hash of the data.
async fn get_data_version_key<S: StoreDriver + ?Sized>(
    store: Pin<&S>,
    key: StoreKey<'_>,
) -> Result<DigestInfo, Error> {
    let mut hasher = DigestHasherFunc::Blake3.hasher();
    let (mut writer, reader) = make_buf_channel_pair();
    // Note: We wrap the future, so if the future is dropped, the writer will be dropped.
    let read_fut = async move { store.get_part(key, &mut writer, 0, None).await };
    let write_fut = async move {
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
    };
    let (read_res, write_res) = futures::join!(read_fut, write_fut);
    if read_res.is_err() {
        return read_res.merge(write_res);
    }
    write_res
}

pub async fn default_store_key_subscribe_with_time<S, SleepFut>(
    store: Arc<S>,
    key: StoreKey<'_>,
    sleep_fn: fn() -> SleepFut,
) -> Box<dyn StoreSubscription>
where
    S: StoreDriver + ?Sized,
    SleepFut: Future<Output = ()> + Send + 'static,
{
    let last_version_key = match get_data_version_key(Pin::new(store.as_ref()), key.borrow()).await
    {
        Ok(version_key) => version_key,
        Err(e) => {
            let (_, watch_rx) = watch::channel(Err(e));
            return Box::new(DefaultStoreSubscription(watch_rx));
        }
    };

    let (watch_tx, watch_rx) = watch::channel::<Result<Arc<dyn StoreSubscriptionItem>, Error>>(Ok(
        Arc::new(DefaultStoreSubscriptionItem {
            store: store.clone(),
            key: key.borrow().into_owned(),
        }),
    ));
    let key = key.borrow().into_owned();

    background_spawn!("default_store_subscribe_spawn", async move {
        let mut last_version_key = last_version_key;
        loop {
            tokio::select! {
                _ = sleep_fn() => {
                    match get_data_version_key(Pin::new(store.as_ref()), key.borrow()).await {
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
    Box::new(DefaultStoreSubscription(watch_rx))
}

/// Default store key subscription implementation.
pub(crate) async fn default_store_key_subscribe<S: StoreDriver + ?Sized>(
    store: Arc<S>,
    key: StoreKey<'_>,
) -> Box<dyn StoreSubscription> {
    default_store_key_subscribe_with_time(store, key, || sleep(DEFAULT_SLEEP_FOR_CHANGES)).await
}
