// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use redis::aio::ConnectionLike;
use redis::{ErrorKind, RedisError, Value};

use crate::redis_utils::aggregate_types::RedisCursorData;

pub(crate) async fn ft_cursor_read<C>(
    connection_manager: &mut C,
    index: String,
    cursor_id: u64,
) -> Result<RedisCursorData, RedisError>
where
    C: ConnectionLike + Send,
{
    let mut cmd = redis::cmd("ft.cursor");
    let ft_cursor_cmd = cmd.arg("read").arg(index).cursor_arg(cursor_id);
    let data = ft_cursor_cmd
        .to_owned()
        .query_async::<Value>(connection_manager)
        .await?;
    let Value::Array(value) = data else {
        return Err(RedisError::from((ErrorKind::Parse, "Expected array")));
    };
    if value.len() < 2 {
        return Err(RedisError::from((
            ErrorKind::Parse,
            "Expected at least 2 elements",
        )));
    }
    let mut value = value.into_iter();
    let Value::Array(data_ary) = value.next().unwrap() else {
        return Err(RedisError::from((ErrorKind::Parse, "Non map item")));
    };
    if data_ary.is_empty() {
        return Err(RedisError::from((
            ErrorKind::Parse,
            "Expected at least 1 element in data array",
        )));
    }
    let Value::Int(new_cursor_id) = value.next().unwrap() else {
        return Err(RedisError::from((
            ErrorKind::Parse,
            "Expected cursor id as second element",
        )));
    };

    Ok(RedisCursorData {
        total: -1, // FIXME(palfrey): fill in value
        cursor: new_cursor_id as u64,
        data: data_ary.into(),
    })
}
