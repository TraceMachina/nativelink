use std::borrow::Borrow;
use std::ops::Bound;
use std::str::from_utf8;
use std::sync::Arc;

use bytes::Bytes;
use fred::prelude::{
    ClientLike, EventInterface, KeysInterface, PubsubInterface, SortedSetsInterface,
    TransactionInterface,
};
use fred::types::{Message, RedisValue, ZRange, ZRangeBound, ZRangeKind, ZSort};
use nativelink_error::{make_err, make_input_err, Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_store::redis_store::RedisStore;
use nativelink_util::action_messages::{ActionInfo, ActionUniqueQualifier, OperationId};
use nativelink_util::background_spawn;
use nativelink_util::store_trait::{StoreKey, StoreLike};
use tracing::{event, Level};

use super::subscription_manager::RedisOperationSubscriber;
use crate::awaited_action_db::{AwaitedAction, SortedAwaitedAction, SortedAwaitedActionState};

#[derive(MetricsComponent)]
pub struct RedisAdapter {
    store: Arc<RedisStore>,
}

pub fn to_redis_bound<T>(rust_bound: Bound<T>, start: bool) -> ZRange
where
    T: ToString,
{
    match rust_bound {
        Bound::Unbounded => {
            let range = {
                if start {
                    ZRangeBound::NegInfinityLex
                } else {
                    ZRangeBound::InfiniteLex
                }
            };
            ZRange {
                kind: ZRangeKind::default(),
                range,
            }
        }
        Bound::Included(v) => ZRange {
            kind: ZRangeKind::Inclusive,
            range: v.to_string().into(),
        },
        Bound::Excluded(v) => ZRange {
            kind: ZRangeKind::Exclusive,
            range: v.to_string().into(),
        },
    }
}

#[derive(Clone, Debug)]
pub enum RedisKeys<'a> {
    AwaitedAction(&'a OperationId),
    OperationIdByClientId(&'a OperationId),
    OperationIdByHashKey(&'a ActionUniqueQualifier),
}

impl<'a> std::fmt::Display for RedisKeys<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AwaitedAction(oid) => f.write_fmt(format_args!("oid:{oid}")),
            Self::OperationIdByClientId(cid) => f.write_fmt(format_args!("cid:{cid}")),
            Self::OperationIdByHashKey(ahk) => f.write_fmt(format_args!("ahk:{ahk}")),
        }
    }
}

impl<'a> From<&RedisKeys<'a>> for StoreKey<'a> {
    fn from(value: &RedisKeys<'a>) -> Self {
        StoreKey::Str(value.to_string().into())
    }
}

// Add a SortedAwaitedAction
impl RedisAdapter {
    pub fn new(store: Arc<RedisStore>) -> Self {
        Self { store }
    }
    pub async fn get_bytes(&self, key: &RedisKeys<'_>) -> Result<Bytes, nativelink_error::Error> {
        let bytes = self.store.get_part_unchunked(key, 0, None).await?;
        if bytes.is_empty() {
            return Err(make_err!(
                Code::NotFound,
                "Failed to find value for key - {key}"
            ));
        }
        Ok(bytes)
    }
    pub async fn subscribe_to_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<tokio::sync::watch::Receiver<AwaitedAction>, Error> {
        println!("in subscribe_to_operation");
        let client = self.store.get_subscriber_client();
        client.init().await?;
        let key = RedisKeys::AwaitedAction(operation_id);
        let bytes = self
            .get_bytes(&key)
            .await
            .err_tip(|| "In RedisAdapter::subscribe_to_operation")?;
        let initial_state = AwaitedAction::try_from(bytes.as_ref())?;
        let update_channel = format!("updates:{key}").to_string();
        let (tx, mut rx) = tokio::sync::watch::channel(initial_state);
        rx.mark_changed();
        let _ = background_spawn!("spawn", async move {
            println!("subscribing with client");
            client.subscribe(update_channel.clone()).await.unwrap();
            println!("subscribed with client");
            println!("subscribed to channel - {update_channel}");
            // Max capacity.
            let handler = move |event: Message| {
                println!("{:?}", event.channel);
                if let Some(bytes) = event.value.as_bytes() {
                    let awaited_action_result = AwaitedAction::try_from(bytes);
                    match awaited_action_result {
                        Ok(awaited_action) => {
                            tx.send_replace(awaited_action);
                        }
                        Err(e) => {
                            event!(
                                Level::ERROR,
                                ?e,
                                "Failed to decode awaited action from redis"
                            );
                        }
                    }
                } else {
                    event!(Level::ERROR, "Recieved event without message");
                }
            };
            let _ = client
                .on_message(move |message| {
                    handler(message);
                    Ok(())
                })
                .await;
        })
        .await;

        Ok(rx)
    }

    async fn add_to_set(&self, set: &str, key: &str) -> Result<(), Error> {
        let client = self.store.get_client();
        let tx = client.multi();
        let _: RedisValue = tx.set(key, set, None, None, false).await?;
        let _: RedisValue = tx
            .zadd(
                set,
                Some(fred::types::SetOptions::NX),
                None,
                false,
                false,
                (0 as f64, key),
            )
            .await?;
        // Watch before does not currently work with `ClientPool`
        // See: https://github.com/aembke/fred.rs/issues/251
        tx.watch_before(key);
        let _: RedisValue = tx.exec(true).await?;
        // Only succeeds if element doesn't already exist.
        Ok(())
    }

    async fn move_or_add_to_set(&self, to: &str, key: &str) -> Result<(), Error> {
        let client = self.store.get_client();
        // Get the set the value is currently in
        // If this value changes before the tx finishes, the transaction reverts.
        let maybe_current_set: Option<String> = client.get(key).await?;
        let tx = client.multi();
        tx.watch_before(key);

        if let Some(current_set) = maybe_current_set {
            let _: RedisValue = tx.zrem(current_set, key).await?;
        }

        let _: RedisValue = tx
            .zadd(to, None, None, false, false, (0 as f64, key))
            .await?;
        let _: RedisValue = tx.set(key, to, None, None, false).await?;
        Ok(tx.exec(true).await?)
    }

    pub async fn list_set<T>(
        &self,
        set: &str,
        start: Bound<T>,
        end: Bound<T>,
        desc: bool,
    ) -> Result<Vec<String>, Error>
    where
        T: ToString,
    {
        let start_bound = to_redis_bound(start, true);
        let end_bound = to_redis_bound(end, false);
        let client = self.store.get_client();
        Ok(client
            .zrange(
                set,
                start_bound,
                end_bound,
                Some(ZSort::ByLex),
                desc,
                None,
                false,
            )
            .await?)
    }

    pub async fn get_range_of_actions(
        &self,
        state: SortedAwaitedActionState,
        start: Bound<SortedAwaitedAction>,
        end: Bound<SortedAwaitedAction>,
        desc: bool,
    ) -> Result<Vec<Result<RedisOperationSubscriber, Error>>, Error> {
        let mut output: Vec<Result<RedisOperationSubscriber, Error>> = Vec::new();
        let sorted_actions_result = self.list_set(&state.to_string(), start, end, desc).await;
        if let Ok(sorted_actions_bytes) = sorted_actions_result {
            let sorted_actions_results: Vec<Result<SortedAwaitedAction, Error>> =
                sorted_actions_bytes
                    .iter()
                    .map(|bytes| SortedAwaitedAction::try_from(bytes.as_bytes()))
                    .collect();
            for result in sorted_actions_results {
                match result {
                    Ok(sorted_action) => {
                        let sub_result = self
                            .subscribe_to_operation(&sorted_action.operation_id)
                            .await;
                        match sub_result {
                            Ok(sub) => output.push(Ok(RedisOperationSubscriber {
                                awaited_action_rx: sub,
                            })),
                            Err(e) => output.push(Err(e)),
                        }
                    }
                    Err(e) => output.push(Err(e)),
                }
            }
        };
        Ok(output)
    }
    pub async fn list_all_sorted_actions(
        &self,
    ) -> Result<Vec<Result<RedisOperationSubscriber, Error>>, Error> {
        let mut output: Vec<Result<RedisOperationSubscriber, Error>> = Vec::new();
        let states = &[
            SortedAwaitedActionState::CacheCheck,
            SortedAwaitedActionState::Completed,
            SortedAwaitedActionState::Executing,
            SortedAwaitedActionState::Queued,
        ];
        for state in states.iter() {
            let sorted_actions_result = self
                .list_set(
                    &state.to_string(),
                    Bound::Unbounded::<SortedAwaitedAction>,
                    Bound::Unbounded::<SortedAwaitedAction>,
                    false,
                )
                .await;

            if let Ok(sorted_actions_bytes) = sorted_actions_result {
                let sorted_actions_results: Vec<Result<SortedAwaitedAction, Error>> =
                    sorted_actions_bytes
                        .iter()
                        .map(|bytes| SortedAwaitedAction::try_from(bytes.as_bytes()))
                        .collect();
                for result in sorted_actions_results {
                    match result {
                        Ok(sorted_action) => {
                            let sub_result = self
                                .subscribe_to_operation(&sorted_action.operation_id)
                                .await;
                            match sub_result {
                                Ok(sub) => output.push(Ok(RedisOperationSubscriber {
                                    awaited_action_rx: sub,
                                })),
                                Err(e) => output.push(Err(e)),
                            }
                        }
                        Err(e) => output.push(Err(e)),
                    }
                }
            } else {
                continue;
            }
        }
        Ok(output)
    }

    pub async fn get_awaited_action(
        &self,
        operation_id: &OperationId,
    ) -> Result<AwaitedAction, Error> {
        let bytes = self
            .get_bytes(&RedisKeys::AwaitedAction(operation_id))
            .await?;
        serde_json::from_slice(&bytes).map_err(|err| {
            make_input_err!("In RedisAdapter::get_awaited_action - {}", err.to_string())
        })
    }

    pub async fn get_operation_id_by_client_id(
        &self,
        client_id: &OperationId,
    ) -> Result<OperationId, Error> {
        let bytes = self
            .get_bytes(&RedisKeys::OperationIdByClientId(client_id))
            .await?;
        Ok(OperationId::from_raw_string(
            from_utf8(&bytes)
                .map_err(|err| {
                    make_input_err!(
                        "In RedisAdapter::get_operation_id_by_client_id - {}",
                        err.to_string()
                    )
                })?
                .to_string(),
        ))
    }

    pub async fn get_operation_id_by_action_hash_key(
        &self,
        unique_qualifier: &ActionUniqueQualifier,
    ) -> Result<OperationId, Error> {
        let bytes = self
            .get_bytes(&RedisKeys::OperationIdByHashKey(unique_qualifier))
            .await?;
        Ok(OperationId::from_raw_string(
            from_utf8(&bytes)
                .map_err(|err| {
                    make_input_err!(
                        "In RedisAdapter::get_operation_id_by_action_hash_key - {}",
                        err.to_string()
                    )
                })?
                .to_string(),
        ))
    }

    async fn add_new_operation(
        &self,
        client_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<tokio::sync::watch::Receiver<AwaitedAction>, Error> {
        let operation_id = OperationId::default();

        let action = AwaitedAction::new(operation_id.clone(), action_info.clone());
        let awaited_action_key = RedisKeys::AwaitedAction(&operation_id);
        let operation_id_cid = RedisKeys::OperationIdByClientId(&client_id);
        let operation_id_ahk = RedisKeys::OperationIdByHashKey(&action_info.unique_qualifier);

        let sorted_state = SortedAwaitedActionState::try_from(action.state().stage.clone())?;
        let sorted_action: SortedAwaitedAction = action.borrow().into();
        let action_bytes: Bytes = action.clone().try_into()?;

        self.add_to_set(&sorted_state.to_string(), &sorted_action.to_string())
            .await?;
        self.store
            .update_oneshot(&awaited_action_key, action_bytes)
            .await?;
        self.store
            .update_oneshot(&operation_id_cid, operation_id.to_string().into())
            .await?;
        self.store
            .update_oneshot(&operation_id_ahk, operation_id.to_string().into())
            .await?;
        self.subscribe_to_operation(&operation_id).await
    }

    pub async fn subscribe_client_to_operation(
        &self,
        client_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<tokio::sync::watch::Receiver<AwaitedAction>, Error> {
        println!("subscribe client to operation");
        match self.get_operation_id_by_client_id(&client_id).await {
            Ok(operation_id) => {
                let sub = self.subscribe_to_operation(&operation_id)
                    .await
                    .err_tip(|| "In RedisAwaitedActionDb::subscribe_client_to_operation - subscribe_to_operation")?;
                self.store
                    .update_oneshot(
                        &RedisKeys::OperationIdByClientId(&client_id),
                        operation_id.to_string().into(),
                    )
                    .await
                    .err_tip(|| {
                        "In RedisAwaitedActionDb::subscribe_client_to_operation - update_oneshot"
                    })?;
                Ok(sub)
            }
            Err(e) => match e.code {
                Code::NotFound => Ok(self
                    .add_new_operation(client_id, action_info)
                    .await
                    .err_tip(|| {
                        "In RedisAwaitedActionDb::subscribe_client_to_operation - add_new_operation"
                    })?),
                _ => {
                    println!("ERR UNHANDLED");
                    Err(e)
                }
            },
        }
    }

    /// Process a change changed AwaitedAction and notify any listeners.
    pub async fn update_awaited_action(
        &self,
        new_awaited_action: AwaitedAction,
    ) -> Result<(), Error> {
        let client = self.store.get_client();
        let operation_id = new_awaited_action.operation_id().clone();
        let new_sorted_state =
            SortedAwaitedActionState::try_from(&new_awaited_action.state().stage)?;
        let sorted_awaited_action = SortedAwaitedAction::from(&new_awaited_action);
        self.move_or_add_to_set(
            &new_sorted_state.to_string(),
            &sorted_awaited_action.to_string(),
        )
        .await?;
        let awaited_action_bytes: Bytes = new_awaited_action.clone().try_into()?;
        let key = RedisKeys::AwaitedAction(&operation_id);
        let pub_key = format!("updates:{key}");
        println!("pub key - {pub_key}");
        self.store
            .update_oneshot(key.to_string().as_str(), awaited_action_bytes.clone())
            .await?;
        let _: RedisValue = client.publish(pub_key, awaited_action_bytes).await?;
        Ok(())
    }
}
