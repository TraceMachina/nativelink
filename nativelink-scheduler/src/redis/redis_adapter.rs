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
use std::ops::{Bound, RangeBounds};
use std::str::from_utf8;
use std::sync::Arc;

use bytes::Bytes;
use fred::interfaces::{
    KeysInterface, PubsubInterface, SortedSetsInterface, TransactionInterface,
};
use fred::types::{RedisValue, ZRange, ZRangeBound, ZRangeKind, ZSort};
use futures::{future, stream, Stream};
use nativelink_error::{error_if, make_err, make_input_err, Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_store::redis_store::RedisStore;
use nativelink_util::action_messages::{ActionInfo, ActionUniqueQualifier, OperationId};
use nativelink_util::chunked_stream::ChunkedStream;
use nativelink_util::store_trait::{StoreKey, StoreLike};
use tokio::sync::{watch, Notify};
use tracing::{event, Level};

use super::subscription_manager::{RedisOperationSubscriber, RedisOperationSubscribers};
use crate::awaited_action_db::{self, AwaitedAction, SortedAwaitedAction, SortedAwaitedActionState};

#[derive(MetricsComponent)]
pub struct RedisAdapter {
    store: Arc<RedisStore>,
    operation_subscribers: RedisOperationSubscribers,
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
    pub fn new(store: Arc<RedisStore>, tasks_or_workers_change_notify: Arc<Notify>) -> Self {
        Self {
            operation_subscribers: RedisOperationSubscribers::new(store.clone(), tasks_or_workers_change_notify),
            store,
        }
    }
    pub async fn get_bytes(&self, key: &RedisKeys<'_>) -> Result<Bytes, nativelink_error::Error> {
        let bytes = self
            .store
            .get_part_unchunked(key, 0, None)
            .await
            .err_tip(|| "In RedisAdapter::get_bytes")?;
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
    ) -> Result<RedisOperationSubscriber, Error> {
        match self.operation_subscribers
            .get_operation_subscriber(operation_id)
            .await
        {
            Ok(sub) => { Ok(sub) }
            Err(_) => {
                let action = self.get_bytes(&RedisKeys::AwaitedAction(operation_id)).await.and_then(AwaitedAction::try_from).err_tip(|| "In RedisAdapter::subscribe_to_operation - Failed to get subscriber and failed to fetch awaited action from redis")?;
                let tx = watch::Sender::new(action);
                self.operation_subscribers.set_operation_sender(operation_id, tx).await;
                self.operation_subscribers.get_operation_subscriber(operation_id).await
            }

        }
    }

    // Takes the name of a sorted set and the key to add to it.
    // If the key already exists in another set, this will remove it from that set.
    async fn move_or_add_to_set(&self, set_name: &str, key: &str) -> Result<(), Error> {
        let client = self.store.get_client();
        // Get the name of the sorted set the value is currently in
        // If this value changes before the tx finishes, the transaction reverts.
        let maybe_current_set_name: Option<String> = client
            .get(key)
            .await
            .err_tip(|| "In RedisAdapter::move_or_add_to_set")?;

        // Start a transaction
        let tx = client.multi();

        // Revert if the set-tracker key changes.
        // Note: watch_before does not currently work with `ClientPool`
        // This should be fixed upstream in the next release.
        // See: https://github.com/aembke/fred.rs/issues/251
        tx.watch_before(key);

        if let Some(current_set_name) = maybe_current_set_name {
            let _result: RedisValue = tx
                .zrem(current_set_name, key)
                .await
                .err_tip(|| "In RedisAdapter::move_or_add_to_set")?;
        }

        // Add the key to the sorted set
        let _: RedisValue = tx
            .zadd(set_name, None, None, false, false, (0 as f64, key))
            .await
            .err_tip(|| "In RedisAdapter::move_or_add_to_set")?;

        // Store the name of the set the key belongs to in `key` so anyone else
        // trying to update the set will revert.
        let _: RedisValue = tx
            .set(key, set_name, None, None, false)
            .await
            .err_tip(|| "In RedisAdapter::move_or_add_to_set")?;
        tx.exec(true)
            .await
            .err_tip(|| "In RedisAdapter::move_or_add_to_set")
    }

    pub async fn list_set<T>(
        &self,
        set: &str,
        start: Bound<T>,
        end: Bound<T>,
        desc: bool,
    ) -> Result<Vec<Bytes>, Error>
    where
        T: ToString,
    {
        let start_bound = to_redis_bound(start, !desc);
        let end_bound = to_redis_bound(end, desc);
        let client = self.store.get_client();
        client
            .zrange(
                set,
                start_bound,
                end_bound,
                Some(ZSort::ByLex),
                desc,
                None,
                false,
            )
            .await
            .err_tip(|| "In RedisAdapter::list_set")
    }

    pub async fn get_range_of_actions(
        &self,
        state: SortedAwaitedActionState,
        start: Bound<SortedAwaitedAction>,
        end: Bound<SortedAwaitedAction>,
        desc: bool,
    ) -> impl Stream<Item = Result<RedisOperationSubscriber, Error>> + Send + '_ {
        ChunkedStream::new(start, end, move |start, end, mut output| async move {
                let client = self.store.get_client();
                let mut done = true;
                let mut new_start = start.clone();
                let mut new_end = end.clone();
                let Ok(result): Result<Vec<Bytes>, Error> = client
                    .zrange(
                        state.to_string(),
                        to_redis_bound(start.clone(), !desc),
                        to_redis_bound(end.clone(), desc),
                        Some(ZSort::ByLex),
                        desc,
                        Some(fred::types::Limit::from((0, 1))),
                        false
                    ).await
                    .err_tip(|| "In list range")
            else {
                println!("In list actions - error in zrange");
                return Ok(None)
            };
            let sorted_actions_results: Vec<Result<SortedAwaitedAction, Error>> = result.iter().map(SortedAwaitedAction::try_from).collect();
            for sorted_action in sorted_actions_results.into_iter().flatten() {
                new_start = Bound::Excluded(sorted_action.clone());
                match self.operation_subscribers.get_operation_subscriber(&sorted_action.operation_id).await {
                    Ok(tx) => { output.push_back(tx) },
                    _ => {
                        let key = RedisKeys::AwaitedAction(&sorted_action.operation_id);
                        let action_result = self.get_bytes(&key).await.and_then(AwaitedAction::try_from);
                        match action_result {
                            Ok(action) => {
                                let (tx, rx) = watch::channel(action);
                                self.operation_subscribers.set_operation_sender(&sorted_action.operation_id, tx.clone()).await;
                                output.push_back(RedisOperationSubscriber { awaited_action_rx: rx })
                            },
                            Err(e) => {
                                println!("{e:?}")
                            }
                        }
                    }
                }
            }
            Ok(
                Some((
                    (new_start, new_end),
                    output
                ))
            )
        })
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
                        .map(SortedAwaitedAction::try_from)
                        .collect();
                for result in sorted_actions_results {
                    match result {
                        Ok(sorted_action) => {
                            let sub_result = self
                                .subscribe_to_operation(&sorted_action.operation_id)
                                .await;
                            output.push(sub_result);
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
        self.get_bytes(&RedisKeys::AwaitedAction(operation_id))
            .await
            .and_then(AwaitedAction::try_from)
            .err_tip(|| "In RedisAdapter::GetAwaitedAction")
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
    ) -> Result<RedisOperationSubscriber, Error> {
        let operation_id = OperationId::default();

        let action = AwaitedAction::new(operation_id.clone(), action_info.clone());
        let awaited_action_key = RedisKeys::AwaitedAction(&operation_id);
        let operation_id_cid = RedisKeys::OperationIdByClientId(&client_id);
        let operation_id_ahk = RedisKeys::OperationIdByHashKey(&action_info.unique_qualifier);

        let sorted_state = SortedAwaitedActionState::try_from(action.state().stage.clone())?;
        let sorted_action: SortedAwaitedAction = action.borrow().into();
        let action_bytes: Bytes = action.clone().try_into()?;

        // First create a channel in memory with the action state.
        let (tx, rx) = watch::channel(action.clone());
        // Set the subscription sender for the rx.
        self.operation_subscribers
            .set_operation_sender(&operation_id, tx)
            .await;
        let sub = RedisOperationSubscriber::new(rx);

        self.store
            .update_oneshot(&awaited_action_key, action_bytes)
            .await
            .err_tip(|| "In RedisAdapter::add_action")?;
        self.store
            .update_oneshot(&operation_id_cid, operation_id.to_string().into())
            .await
            .err_tip(|| "In RedisAdapter::add_action")?;
        self.store
            .update_oneshot(&operation_id_ahk, operation_id.to_string().into())
            .await
            .err_tip(|| "In RedisAdapter::add_action")?;


        self.move_or_add_to_set(&sorted_state.to_string(), &sorted_action.to_string()).await.err_tip(|| "In RedisAdapter update_awaited_action")?;
        let awaited_action_bytes: Bytes = action.clone().try_into()?;
        let key = RedisKeys::AwaitedAction(&operation_id);
        let pub_key = format!("updates:{key}");
        self.store.update_oneshot(key.to_string().as_str(), awaited_action_bytes.clone()).await.err_tip(|| "In RedisAdapter update_awaited_action")?;
        let client = self.store.get_client();

        let _: RedisValue = client.publish(pub_key, awaited_action_bytes)
            .await
            .err_tip(|| "In RedisAdapter::add_action")?;
        Ok(sub)
    }

    pub async fn subscribe_client_to_operation(
        &self,
        client_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<RedisOperationSubscriber, Error> {
        let operation_id_result = self.get_operation_id_by_client_id(&client_id).await;
        match operation_id_result {
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
                _ => Err(e),
            },
        }
    }

    // pub async fn update_awaited_action(
    //     &self,
    //     new_awaited_action: AwaitedAction,
    // ) -> Result<(), Error> {
    //     let client = self.store.get_client();
    //     let operation_id = new_awaited_action.operation_id().clone();
    //     let new_sorted_state =
    //         SortedAwaitedActionState::try_from(&new_awaited_action.state().stage)
    //             .err_tip(|| "In RedisAdapter::update_awaited_action")?;
    //     let sorted_awaited_action = SortedAwaitedAction::from(&new_awaited_action);
    //
    //     self.move_or_add_to_set(&new_sorted_state.to_string(), &sorted_awaited_action.to_string()).await.err_tip(|| "In RedisAdapter update_awaited_action")?;
    //     let awaited_action_bytes: Bytes = new_awaited_action.clone().try_into()?;
    //     let key = RedisKeys::AwaitedAction(&operation_id);
    //     let pub_key = format!("updates:{key}");
    //     self.store.update_oneshot(key.to_string().as_str(), awaited_action_bytes.clone()).await.err_tip(|| "In RedisAdapter update_awaited_action")?;
    //     let _: RedisValue = client.publish(pub_key, awaited_action_bytes)
    //         .await
    //         .err_tip(|| "In RedisAdapter update_awaited_action")?;
    //     Ok(())
    // }
    /// Process a change changed AwaitedAction and notify any listeners.
    pub async fn update_awaited_action(
        &self,
        new_awaited_action: AwaitedAction,
    ) -> Result<(), Error> {
        let action_key = RedisKeys::AwaitedAction(new_awaited_action.operation_id());
        // Note: We error here if the action is not in redis instead of uploading it
        // because we don't have the client ID and therefore have insufficient information
        // to add the action.
        let old_awaited_action = self
            .get_bytes(&action_key)
            .await
            .and_then(AwaitedAction::try_from)
            .err_tip(|| "In RedisAdapter::update_awaited_action")?;

        let new_awaited_action_bytes: Bytes = new_awaited_action
            .clone()
            .try_into()
            .err_tip(|| "In RedisAdapter::update_awaited_action")?;

        // Don't update if redis action version is newer.
        // This would occur if a pub message from redis with an update was missed.
        if new_awaited_action.version() != old_awaited_action.version()+1 {
            return Err(make_err!(
                // From: https://grpc.github.io/grpc/core/md_doc_statuscodes.html
                // Use ABORTED if the client should retry at a higher level
                // (e.g., when a client-specified test-and-set fails,
                // indicating the client should restart a read-modify-write
                // sequence)
                Code::Aborted,
                "{} Expected {:?} but got {:?} for operation_id {:?} - {:?}",
                "Tried to update an awaited action with an incorrect version.",
                old_awaited_action.version() + 1,
                new_awaited_action.version(),
                old_awaited_action,
                new_awaited_action,
            ));
        }
        error_if!(
            old_awaited_action.action_info().unique_qualifier
                != new_awaited_action.action_info().unique_qualifier,
            "Unique key changed for operation_id {:?} - {:?} - {:?}",
            new_awaited_action.operation_id(),
            old_awaited_action.action_info(),
            new_awaited_action.action_info(),
        );
        let sorted_awaited_action = SortedAwaitedAction::from(&new_awaited_action);

        if new_awaited_action.state() != old_awaited_action.state() {
            let new_sorted_state =
                SortedAwaitedActionState::try_from(&new_awaited_action.state().stage)
                    .err_tip(|| "In RedisAdapter::update_awaited_action")?;
            self.move_or_add_to_set(
                &new_sorted_state.to_string(),
                &sorted_awaited_action.to_string(),
            )
            .await
            .err_tip(|| "In RedisAdapter update_awaited_action")?;
        }
        let pub_key = format!("updates:{action_key}");
        self.store
            .update_oneshot(
                action_key.to_string().as_str(),
                new_awaited_action_bytes.clone(),
            )
            .await
            .err_tip(|| "In RedisAdapter update_awaited_action")?;
        // Publish through redis since we updated to redis.
        let client = self.store.get_client();
        let _: RedisValue = client
            .publish(pub_key, new_awaited_action_bytes)
            .await
            .err_tip(|| "In RedisAdapter update_awaited_action")?;
        Ok(())
    }
}

        // let maybe_tx = self
        //     .operation_subscribers
        //     .get_operation_sender(new_awaited_action.operation_id())
        //     .await;
        //
        // // If our local version is out of date, we should update it to the current one.
        // let tx = match maybe_tx {
        //     Some(tx) => tx,
        //     None => {
        //         // If the action was found in redis and we don't have a local subscription sender,
        //         // then initialize one.
        //         let tx = watch::Sender::new(old_awaited_action.clone());
        //         self.operation_subscribers
        //             .set_operation_sender(old_awaited_action.operation_id(), tx.clone())
        //             .await;
        //         tx
        //     }
        // };
        // tx.send_replace(old_awaited_action.clone());
