// Copyright 2024-2025 The NativeLink Authors. All rights reserved.
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

use futures::Stream;
use nativelink_error::Error;
use redis::aio::ConnectionLike;
use redis::{ErrorKind, RedisError, ToRedisArgs, Value};
use tracing::error;

use crate::redis_utils::aggregate_types::RedisCursorData;
use crate::redis_utils::ft_cursor_read::ft_cursor_read;

pub(crate) struct FtAggregateCursor {
    pub count: u64,
    pub max_idle: u64,
}

pub(crate) struct FtAggregateOptions {
    pub load: Vec<String>,
    pub cursor: FtAggregateCursor,
    pub sort_by: Vec<String>,
}

/// Calls `FT.AGGREGATE` in redis. redis-rs does not properly support this command
/// so we have to manually handle it.
pub(crate) async fn ft_aggregate<C, Q>(
    mut connection_manager: C,
    index: String,
    query: Q,
    options: FtAggregateOptions,
) -> Result<impl Stream<Item = Result<Value, RedisError>> + Send, Error>
where
    Q: ToRedisArgs,
    C: ConnectionLike + Send,
{
    struct State<C: ConnectionLike> {
        connection_manager: C,
        index: String,
        data: RedisCursorData,
    }

    let mut cmd = redis::cmd("FT.AGGREGATE");
    let mut ft_aggregate_cmd = cmd
        .arg(&index)
        .arg(query)
        .arg("LOAD")
        .arg(options.load.len())
        .arg(options.load)
        .arg("WITHCURSOR")
        .arg("COUNT")
        .arg(options.cursor.count)
        .arg("MAXIDLE")
        .arg(options.cursor.max_idle)
        .arg("SORTBY")
        .arg(options.sort_by.len());
    for key in options.sort_by {
        ft_aggregate_cmd = ft_aggregate_cmd.arg(key).arg("ASC");
    }
    let res = ft_aggregate_cmd
        .to_owned()
        .query_async::<Value>(&mut connection_manager)
        .await;
    let data = match res {
        Ok(d) => d,
        Err(e) => {
            error!(?e, "Error calling ft.aggregate");
            return Err(e.into());
        }
    };

    let state = State {
        connection_manager,
        index,
        data: data.try_into()?,
    };

    Ok(futures::stream::unfold(
        Some(state),
        move |maybe_state| async move {
            let mut state = maybe_state?;
            loop {
                if let Some(map) = state.data.data.pop_front() {
                    return Some((Ok(map), Some(state)));
                }
                if state.data.cursor == 0 {
                    return None;
                }
                let data_res = ft_cursor_read(
                    &mut state.connection_manager,
                    state.index.clone(),
                    state.data.cursor,
                )
                .await;
                state.data = match data_res {
                    Ok(data) => data,
                    Err(err) => return Some((Err(err), None)),
                };
            }
        },
    ))
}

impl TryFrom<Value> for RedisCursorData {
    type Error = RedisError;
    fn try_from(raw_value: Value) -> Result<Self, RedisError> {
        let Value::Array(value) = raw_value else {
            return Err(RedisError::from((ErrorKind::ParseError, "Expected array")));
        };
        if value.len() < 2 {
            return Err(RedisError::from((
                ErrorKind::ParseError,
                "Expected at least 2 elements",
            )));
        }
        let mut output = Self::default();
        let mut value = value.into_iter();
        let results_array = match value.next().unwrap() {
            Value::Array(d) => d,
            other => {
                error!(?other, "Bad data in ft.aggregate, expected array");
                return Err(RedisError::from((
                    ErrorKind::ParseError,
                    "Non map item",
                    format!("{other:?}"),
                )));
            }
        };
        let mut results_iter = results_array.iter();
        match results_iter.next() {
            Some(Value::Int(t)) => {
                output.total = *t;
            }
            Some(other) => {
                error!(?other, "Non-int for first value in ft.aggregate");
                return Err(RedisError::from((
                    ErrorKind::ParseError,
                    "Non int for aggregate total",
                    format!("{other:?}"),
                )));
            }
            None => {
                error!("No items in results array for ft.aggregate!");
                return Err(RedisError::from((
                    ErrorKind::ParseError,
                    "No items in results array for ft.aggregate",
                )));
            }
        }

        for item in results_iter {
            match item {
                Value::Array(items) if items.len() == 4 => {}
                other => {
                    error!(
                        ?other,
                        "Expected an array of size 4, didn't get it for aggregate value"
                    );
                    return Err(RedisError::from((
                        ErrorKind::ParseError,
                        "Expected an array of size 4, didn't get it for aggregate value",
                        format!("{other:?}"),
                    )));
                }
            }

            output.data.push_back(item.clone());
        }
        let Value::Int(cursor) = value.next().unwrap() else {
            return Err(RedisError::from((
                ErrorKind::ParseError,
                "Expected integer as last element",
            )));
        };
        output.cursor = cursor as u64;
        Ok(output)
    }
}
