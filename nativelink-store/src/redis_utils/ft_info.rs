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
    connection_manager: C,
    index: &str,
) -> Result<bool, RedisError>
where
    C: ConnectionLike + Send,
{
    Ok(ft_info(connection_manager, index).await?.indexing)
}

/// The parts of `FT.INFO` that say whether an index holds every key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FtInfo {
    /// The background scan that follows `FT.CREATE` is still running.
    pub(crate) indexing: bool,
    /// Keys the index holds.
    pub(crate) num_docs: u64,
    /// Keys the index failed to take, for instance on running out of
    /// memory. A scan can stop with `indexing 0` and keys missing; this and
    /// `num_docs` are how that shows. Zero when the server does not report it.
    pub(crate) hash_indexing_failures: u64,
}

pub(crate) async fn ft_info<C>(mut connection_manager: C, index: &str) -> Result<FtInfo, RedisError>
where
    C: ConnectionLike + Send,
{
    let value = redis::cmd("FT.INFO")
        .arg(index)
        .query_async::<Value>(&mut connection_manager)
        .await?;
    parse_ft_info(&value)
}

/// `FT.INFO` answers a flat key-value array on RESP2 and a map on RESP3.
fn parse_ft_info(value: &Value) -> Result<FtInfo, RedisError> {
    let indexing = number(value, "indexing").ok_or_else(|| {
        RedisError::from((ErrorKind::Parse, "FT.INFO reply carries no indexing field"))
    })?;
    let num_docs = number(value, "num_docs").ok_or_else(|| {
        RedisError::from((ErrorKind::Parse, "FT.INFO reply carries no num_docs field"))
    })?;
    Ok(FtInfo {
        indexing: indexing != 0,
        num_docs,
        hash_indexing_failures: number(value, "hash_indexing_failures").unwrap_or(0),
    })
}

/// A top-level numeric field of an `FT.INFO` reply, which `RediSearch` sends
/// as an integer, a string or a double depending on the field and protocol.
fn number(value: &Value, wanted: &str) -> Option<u64> {
    let name = |key: &Value| match key {
        Value::BulkString(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        Value::SimpleString(text) => Some(text.clone()),
        _ => None,
    };
    let parse = |value: &Value| match value {
        Value::Int(n) => u64::try_from(*n).ok(),
        Value::BulkString(bytes) => parse_text(&String::from_utf8_lossy(bytes)),
        Value::SimpleString(text) => parse_text(text),
        Value::Double(n) => parse_text(&n.to_string()),
        _ => None,
    };
    match value {
        Value::Array(items) => items.chunks(2).find_map(|pair| match pair {
            [key, value] if name(key).as_deref() == Some(wanted) => parse(value),
            _ => None,
        }),
        Value::Map(entries) => entries.iter().find_map(|(key, value)| {
            (name(key).as_deref() == Some(wanted))
                .then(|| parse(value))
                .flatten()
        }),
        _ => None,
    }
}

fn parse_text(text: &str) -> Option<u64> {
    // Counts only. A count rendered as a float ("12" or "12.0") is a whole
    // number; anything with a real fraction is not a count.
    match text.split_once('.') {
        None => text.parse().ok(),
        Some((whole, fraction)) if fraction.bytes().all(|b| b == b'0') => whole.parse().ok(),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bulk(text: &str) -> Value {
        Value::BulkString(text.as_bytes().to_vec())
    }

    #[test]
    fn reads_the_fields_in_both_protocols() {
        let resp2 = Value::Array(vec![
            bulk("index_name"),
            bulk("i"),
            bulk("num_docs"),
            bulk("26150"),
            bulk("hash_indexing_failures"),
            Value::Int(3),
            bulk("indexing"),
            Value::Int(0),
        ]);
        assert_eq!(
            parse_ft_info(&resp2).unwrap(),
            FtInfo {
                indexing: false,
                num_docs: 26_150,
                hash_indexing_failures: 3
            }
        );
        let resp3 = Value::Map(vec![
            (Value::SimpleString("num_docs".into()), Value::Double(12.0)),
            (Value::SimpleString("indexing".into()), Value::Int(1)),
        ]);
        assert_eq!(
            parse_ft_info(&resp3).unwrap(),
            FtInfo {
                indexing: true,
                num_docs: 12,
                hash_indexing_failures: 0
            }
        );
        assert!(parse_ft_info(&Value::Array(vec![bulk("num_docs"), bulk("1")])).is_err());
    }
}
