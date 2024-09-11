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

use std::collections::VecDeque;

use fred::error::{RedisError, RedisErrorKind};
use fred::interfaces::RediSearchInterface;
use fred::types::{FromRedis, RedisMap, RedisValue};
use futures::Stream;

/// Calls FT_AGGREGATE in redis. Fred does not properly support this command
/// so we have to manually handle it.
pub async fn ft_aggregate<C, I, Q>(
    client: C,
    index: I,
    query: Q,
    options: fred::types::FtAggregateOptions,
) -> Result<impl Stream<Item = Result<RedisMap, RedisError>> + Send, RedisError>
where
    C: RediSearchInterface,
    I: Into<bytes_utils::string::Str>,
    Q: Into<bytes_utils::string::Str>,
{
    let index = index.into();
    let query = query.into();
    let data: RedisCursorData = client.ft_aggregate(index.clone(), query, options).await?;

    struct State<C: RediSearchInterface> {
        client: C,
        index: bytes_utils::string::Str,
        data: RedisCursorData,
    }

    let state = State {
        client,
        index,
        data,
    };
    Ok(futures::stream::unfold(
        Some(state),
        move |maybe_state| async move {
            let mut state = maybe_state?;
            loop {
                match state.data.data.pop_front() {
                    Some(map) => {
                        return Some((Ok(map), Some(state)));
                    }
                    None => {
                        if state.data.cursor == 0 {
                            return None;
                        }
                        let data_res = state
                            .client
                            .ft_cursor_read(state.index.clone(), state.data.cursor, None)
                            .await;
                        state.data = match data_res {
                            Ok(data) => data,
                            Err(err) => return Some((Err(err), None)),
                        };
                        continue;
                    }
                }
            }
        },
    ))
}

#[derive(Debug, Default)]
struct RedisCursorData {
    total: u64,
    cursor: u64,
    data: VecDeque<RedisMap>,
}

impl FromRedis for RedisCursorData {
    fn from_value(value: RedisValue) -> Result<Self, RedisError> {
        if !value.is_array() {
            return Err(RedisError::new(RedisErrorKind::Protocol, "Expected array"));
        }
        let mut output = Self::default();
        let value = value.into_array();
        if value.len() < 2 {
            return Err(RedisError::new(
                RedisErrorKind::Protocol,
                "Expected at least 2 elements",
            ));
        }
        let mut value = value.into_iter();
        let data_ary = value.next().unwrap().into_array();
        if data_ary.is_empty() {
            return Err(RedisError::new(
                RedisErrorKind::Protocol,
                "Expected at least 1 element in data array",
            ));
        }
        let Some(total) = data_ary[0].as_u64() else {
            return Err(RedisError::new(
                RedisErrorKind::Protocol,
                "Expected integer as first element",
            ));
        };
        output.total = total;
        output.data.reserve(data_ary.len() - 1);
        for map_data in data_ary.into_iter().skip(1) {
            output.data.push_back(map_data.into_map()?);
        }
        let Some(cursor) = value.next().unwrap().as_u64() else {
            return Err(RedisError::new(
                RedisErrorKind::Protocol,
                "Expected integer as last element",
            ));
        };
        output.cursor = cursor;
        Ok(output)
    }
}
