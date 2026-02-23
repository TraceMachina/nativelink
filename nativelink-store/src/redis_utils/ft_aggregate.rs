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

use core::fmt::Debug;

use futures::Stream;
use nativelink_error::Error;
use redis::aio::ConnectionLike;
use redis::{Arg, ErrorKind, RedisError, Value};
use tracing::error;

use crate::redis_utils::aggregate_types::RedisCursorData;
use crate::redis_utils::ft_cursor_read::ft_cursor_read;

#[derive(Debug)]
pub(crate) struct FtAggregateCursor {
    pub count: u64,
    pub max_idle: u64,
}

#[derive(Debug)]
pub(crate) struct FtAggregateOptions {
    pub load: Vec<String>,
    pub cursor: FtAggregateCursor,
    pub sort_by: Vec<String>,
}

/// Calls `FT.AGGREGATE` in redis. redis-rs does not properly support this command
/// so we have to manually handle it.
pub(crate) async fn ft_aggregate<C>(
    mut connection_manager: C,
    index: String,
    query: String,
    options: FtAggregateOptions,
) -> Result<impl Stream<Item = Result<Value, RedisError>> + Send, Error>
where
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
        .arg(&query)
        .arg("LOAD")
        .arg(options.load.len())
        .arg(&options.load)
        .arg("WITHCURSOR")
        .arg("COUNT")
        .arg(options.cursor.count)
        .arg("MAXIDLE")
        .arg(options.cursor.max_idle)
        .arg("SORTBY")
        .arg(options.sort_by.len() * 2);
    for key in &options.sort_by {
        ft_aggregate_cmd = ft_aggregate_cmd.arg(key).arg("ASC");
    }
    let res = ft_aggregate_cmd
        .query_async::<Value>(&mut connection_manager)
        .await;
    let data = match res {
        Ok(d) => d,
        Err(e) => {
            let all_args: Vec<_> = ft_aggregate_cmd
                .args_iter()
                .map(|a| match a {
                    Arg::Simple(bytes) => match str::from_utf8(bytes) {
                        Ok(s) => s.to_string(),
                        Err(_) => format!("{bytes:?}"),
                    },
                    other => {
                        format!("{other:?}")
                    }
                })
                .collect();
            error!(
                ?e,
                index,
                ?query,
                ?options,
                ?all_args,
                "Error calling ft.aggregate"
            );
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

fn resp2_data_parse(
    output: &mut RedisCursorData,
    results_array: &[Value],
) -> Result<(), RedisError> {
    let mut results_iter = results_array.iter();
    match results_iter.next() {
        Some(Value::Int(t)) => {
            output.total = *t;
        }
        Some(other) => {
            error!(?other, "Non-int for first value in ft.aggregate");
            return Err(RedisError::from((
                ErrorKind::Parse,
                "Non int for aggregate total",
                format!("{other:?}"),
            )));
        }
        None => {
            error!("No items in results array for ft.aggregate!");
            return Err(RedisError::from((
                ErrorKind::Parse,
                "No items in results array for ft.aggregate",
            )));
        }
    }

    for item in results_iter {
        match item {
            Value::Array(items) if items.len() % 2 == 0 => {}
            other => {
                error!(
                    ?other,
                    "Expected an array with an even number of items, didn't get it for aggregate value"
                );
                return Err(RedisError::from((
                    ErrorKind::Parse,
                    "Expected an array with an even number of items, didn't get it for aggregate value",
                    format!("{other:?}"),
                )));
            }
        }

        output.data.push_back(item.clone());
    }
    Ok(())
}

fn resp3_data_parse(
    output: &mut RedisCursorData,
    results_map: &Vec<(Value, Value)>,
) -> Result<(), RedisError> {
    for (raw_key, value) in results_map {
        let Value::SimpleString(key) = raw_key else {
            return Err(RedisError::from((
                ErrorKind::Parse,
                "Expected SimpleString keys",
                format!("{raw_key:?}"),
            )));
        };
        match key.as_str() {
            "attributes" => {
                let Value::Array(attributes) = value else {
                    return Err(RedisError::from((
                        ErrorKind::Parse,
                        "Expected array for attributes",
                        format!("{value:?}"),
                    )));
                };
                if !attributes.is_empty() {
                    return Err(RedisError::from((
                        ErrorKind::Parse,
                        "Expected empty attributes",
                        format!("{attributes:?}"),
                    )));
                }
            }
            "format" => {
                let Value::SimpleString(format) = value else {
                    return Err(RedisError::from((
                        ErrorKind::Parse,
                        "Expected SimpleString for format",
                        format!("{value:?}"),
                    )));
                };
                if format.as_str() != "STRING" {
                    return Err(RedisError::from((
                        ErrorKind::Parse,
                        "Expected STRING format",
                        format.clone(),
                    )));
                }
            }
            "results" => {
                let Value::Array(values) = value else {
                    return Err(RedisError::from((
                        ErrorKind::Parse,
                        "Expected Array for results",
                        format!("{value:?}"),
                    )));
                };
                for raw_value in values {
                    let Value::Map(value) = raw_value else {
                        return Err(RedisError::from((
                            ErrorKind::Parse,
                            "Expected list of maps in result",
                            format!("{raw_value:?}"),
                        )));
                    };
                    for (raw_map_key, raw_map_value) in value {
                        let Value::SimpleString(map_key) = raw_map_key else {
                            return Err(RedisError::from((
                                ErrorKind::Parse,
                                "Expected SimpleString keys for result maps",
                                format!("{raw_key:?}"),
                            )));
                        };
                        match map_key.as_str() {
                            "extra_attributes" => {
                                let Value::Map(extra_attributes_values) = raw_map_value else {
                                    return Err(RedisError::from((
                                        ErrorKind::Parse,
                                        "Expected Map for extra_attributes",
                                        format!("{raw_map_value:?}"),
                                    )));
                                };
                                let mut output_array = vec![];
                                for (e_key, e_value) in extra_attributes_values {
                                    output_array.push(e_key.clone());
                                    output_array.push(e_value.clone());
                                }
                                output.data.push_back(Value::Array(output_array));
                            }
                            "values" => {
                                let Value::Array(values_values) = raw_map_value else {
                                    return Err(RedisError::from((
                                        ErrorKind::Parse,
                                        "Expected Array for values",
                                        format!("{raw_map_value:?}"),
                                    )));
                                };
                                if !values_values.is_empty() {
                                    return Err(RedisError::from((
                                        ErrorKind::Parse,
                                        "Expected empty values (all in extra_attributes)",
                                        format!("{values_values:?}"),
                                    )));
                                }
                            }
                            _ => {
                                return Err(RedisError::from((
                                    ErrorKind::Parse,
                                    "Unknown result map key",
                                    format!("{map_key:?}"),
                                )));
                            }
                        }
                    }
                }
            }
            "total_results" => {
                let Value::Int(total) = value else {
                    return Err(RedisError::from((
                        ErrorKind::Parse,
                        "Expected int for total_results",
                        format!("{value:?}"),
                    )));
                };
                output.total = *total;
            }
            "warning" => {
                let Value::Array(warnings) = value else {
                    return Err(RedisError::from((
                        ErrorKind::Parse,
                        "Expected Array for warning",
                        format!("{value:?}"),
                    )));
                };
                if !warnings.is_empty() {
                    return Err(RedisError::from((
                        ErrorKind::Parse,
                        "Expected empty warnings",
                        format!("{warnings:?}"),
                    )));
                }
            }
            _ => {
                return Err(RedisError::from((
                    ErrorKind::Parse,
                    "Unexpected key in ft.aggregate",
                    format!("{key} => {value:?}"),
                )));
            }
        }
    }
    Ok(())
}

impl TryFrom<Value> for RedisCursorData {
    type Error = RedisError;
    fn try_from(raw_value: Value) -> Result<Self, RedisError> {
        let Value::Array(value) = raw_value else {
            error!(
                ?raw_value,
                "Bad data in ft.aggregate, expected array at top-level"
            );
            return Err(RedisError::from((ErrorKind::Parse, "Expected array")));
        };
        if value.len() < 2 {
            return Err(RedisError::from((
                ErrorKind::Parse,
                "Expected at least 2 elements",
            )));
        }
        let mut output = Self::default();
        let mut value = value.into_iter();
        match value.next().unwrap() {
            Value::Array(d) => resp2_data_parse(&mut output, &d)?,
            Value::Map(d) => resp3_data_parse(&mut output, &d)?,
            other => {
                error!(
                    ?other,
                    "Bad data in ft.aggregate, expected array for results"
                );
                return Err(RedisError::from((
                    ErrorKind::Parse,
                    "Non map item",
                    format!("{other:?}"),
                )));
            }
        }
        let Value::Int(cursor) = value.next().unwrap() else {
            return Err(RedisError::from((
                ErrorKind::Parse,
                "Expected integer as last element",
            )));
        };
        output.cursor = cursor as u64;
        Ok(output)
    }
}
