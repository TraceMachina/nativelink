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

use std::task::Poll;

use futures::poll;
use nativelink_error::Code;
use nativelink_macro::nativelink_test;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::default_store_key_subscribe::default_store_key_subscribe_with_time;
use nativelink_util::store_trait::StoreLike;
use pretty_assertions::assert_eq;
use tokio::task::yield_now;

#[nativelink_test]
async fn subscribe_get_key_test() -> Result<(), Box<dyn std::error::Error>> {
    const KEY: &str = "foo";
    let store = MemoryStore::new(&nativelink_config::stores::MemoryStore::default());
    store.update_oneshot(KEY, "bar".into()).await?;
    let subscribe_receiver =
        default_store_key_subscribe_with_time(store, KEY.into(), yield_now).await;
    let subscription_item = subscribe_receiver.peek().unwrap();
    assert_eq!(subscription_item.get_key().await, Ok(KEY.into()));
    Ok(())
}

#[nativelink_test]
async fn subscribe_get_new_versions_test() -> Result<(), Box<dyn std::error::Error>> {
    const KEY: &str = "foo";
    const DATA1: &str = "bar";
    const DATA2: &str = "baz";
    let store = MemoryStore::new(&nativelink_config::stores::MemoryStore::default());
    store.update_oneshot(KEY, DATA1.into()).await?;
    let mut subscribe_receiver =
        default_store_key_subscribe_with_time(store.clone(), KEY.into(), yield_now).await;
    {
        // Test DATA1 ("bar") version.
        let subscription_item = subscribe_receiver.peek().unwrap();
        let (mut tx, mut rx) = make_buf_channel_pair();
        let read_fut = subscription_item.get(&mut tx);
        {
            assert_eq!(poll!(read_fut), Poll::Ready(Ok(())));
            assert_eq!(rx.recv().await, Ok(DATA1.into()));
        }
    }
    {
        // Change the data version.
        let change_fut = subscribe_receiver.changed();
        tokio::pin!(change_fut);
        assert!(poll!(&mut change_fut).is_pending()); // Value should not have changed yet.
        store.update_oneshot(KEY, DATA2.into()).await?;
        change_fut.await.unwrap(); // Wait for the change (happens in another thread).
    }
    {
        // Test DATA2 ("baz") version.
        let subscription_item = subscribe_receiver.peek().unwrap();
        let (mut tx, mut rx) = make_buf_channel_pair();
        let read_fut = subscription_item.get(&mut tx);
        {
            assert_eq!(poll!(read_fut), Poll::Ready(Ok(())));
            assert_eq!(rx.recv().await, Ok(DATA2.into()));
        }
    }
    Ok(())
}

#[nativelink_test]
async fn subscribe_not_found_key_test() -> Result<(), Box<dyn std::error::Error>> {
    let store = MemoryStore::new(&nativelink_config::stores::MemoryStore::default());
    let data = default_store_key_subscribe_with_time(store, "foo".into(), yield_now).await;
    {
        let subscription_err = data.peek().err().unwrap();
        assert_eq!(subscription_err.code, Code::NotFound);
    }
    Ok(())
}
