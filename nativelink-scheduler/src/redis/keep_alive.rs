use std::{sync::Arc, time::Duration};

use fred::{interfaces::KeysInterface, types::Expiration};
use futures::future::BoxFuture;
use mock_instant::SystemTime;
use nativelink_error::{make_err, Code, ResultExt, Error};
use nativelink_store::redis_store::RedisStore;
use nativelink_util::{spawn, store_trait::StoreLike, task::JoinHandleDropGuard};
use uuid::Uuid;
use bytes::Bytes;

type SleepFn = fn(Duration) -> BoxFuture<'static, ()>;
type NowFn = fn() -> SystemTime;

pub struct OwnershipHandler {
    _join_handle: JoinHandleDropGuard<()>,
    store: Arc<RedisStore>,
    id: Uuid,
}

fn owner_key(key: &str) -> Bytes {
    format!("own:{key}")
}

// Send keepalive message every 5s

impl OwnershipHandler {
    pub fn new(store: Arc<RedisStore>, sleep_fn: SleepFn, now_fn: NowFn) -> Self {
        let store_clone = store.clone();
        let id = Uuid::new_v4();
        let id_clone = id.clone();
        let _join_handle = spawn!("ownership_keepalive_spawn", async move {
            loop {
                sleep_fn.await;
                store_clone.update_oneshot(format!("sid:{id}"), now_fn()).await;
            }
        });
        Self {
            _join_handle,
            store,
            id
        }
    }

    pub async fn has_owner(&self, key: &str) -> Result<bool, Error> {
        let client = self.store.get_client();
        let has_owner = client.exists(owner_key(key)).await.err_tip(|| "In OwnershipHandler::has_owner")?;
        Ok(has_owner > 0)
    }

    pub async fn get_owner(&self, key: &str) -> Result<String, Error> {
        let client = self.store.get_client();
        let owner: Option<String> = client
            .get(owner_key(key))
            .await
            .err_tip(|| "In OwnershipHandler::get_owner")?;
    }
    // Takes ownership over key. Fails if key already has an owner.
    pub async fn acquire(&self, key: &str) -> Result<(), Error> {
        if self.has_owner(key).await.err_tip(|| "In OwnershipHandler::acquire")?
        {
            return Err(make_err!(Code::Internal, "Key: {key} is locked")).err_tip(|| "In OwnershipHandler::acquire")
        }
        let client = self.store.get_client();
        client.set(
            owner_key(key),
            self.id,
            Some(Expiration::EX(15)),
            Some(fred::types::SetOptions::NX),
            false
        ).await.err_tip(|| "In OwnershipHandler::get_owner")
    }

    // Takes ownership over key. Fails if key already has an owner.
    pub async fn release(&self, key: &str) -> Result<(), Error> {
        if self.id != self.get_owner(key).await.err_tip(|| "In OwnershipHandler::acquire")?
        {
            return Err(make_err!(Code::Internal, "Key: {key} is locked")).err_tip(|| "In OwnershipHandler::acquire")
        }
        let client = self.store.get_client();
        client.del(
            owner_key(key),
        ).await.err_tip(|| "In OwnershipHandler::get_owner")
    }
}
