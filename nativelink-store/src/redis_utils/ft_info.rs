// Copyright 2026 The NativeLink Authors. All rights reserved.
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

/// Whether `RediSearch` is still backfilling `index`: `FT.INFO`'s
/// `indexing` field, `1` while the background scan that follows `FT.CREATE`
/// is running and `0` once every existing key is in the index. An aggregate
/// issued before that reads a partial index, which for the scheduler's queue
/// is a queue with actions missing from it.
pub(crate) async fn ft_info_indexing<C>(
    mut connection_manager: C,
    index: &str,
) -> Result<bool, RedisError>
where
    C: ConnectionLike + Send,
{
    let value = redis::cmd("FT.INFO")
        .arg(index)
        .query_async::<Value>(&mut connection_manager)
        .await?;
    parse_indexing(&value)
}

/// `FT.INFO` answers a flat key-value array on RESP2 and a map on RESP3.
fn parse_indexing(value: &Value) -> Result<bool, RedisError> {
    let field = |key: &Value, value: &Value| -> Option<bool> {
        let name = match key {
            Value::BulkString(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            Value::SimpleString(text) => text.clone(),
            _ => return None,
        };
        if name != "indexing" {
            return None;
        }
        match value {
            Value::Int(n) => Some(*n != 0),
            Value::BulkString(bytes) => Some(bytes.as_slice() != b"0"),
            Value::SimpleString(text) => Some(text != "0"),
            _ => None,
        }
    };
    match value {
        Value::Array(items) => {
            for pair in items.chunks(2) {
                if let [key, value] = pair
                    && let Some(indexing) = field(key, value)
                {
                    return Ok(indexing);
                }
            }
        }
        Value::Map(entries) => {
            for (key, value) in entries {
                if let Some(indexing) = field(key, value) {
                    return Ok(indexing);
                }
            }
        }
        _ => {}
    }
    Err(RedisError::from((
        ErrorKind::Parse,
        "FT.INFO reply carries no indexing field",
    )))
}
