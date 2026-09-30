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

use redis::aio::ConnectionLike;
use redis::{ErrorKind, RedisError, Value};
use tracing::error;

/// Per-query `FT.SEARCH` timeout in milliseconds. Matches the `FT.AGGREGATE`
/// timeout: the index is the same size, so the scan can take just as long.
const FT_SEARCH_TIMEOUT_MS: u64 = 10_000;

/// Counts the documents matching a query without fetching any of them.
///
/// `LIMIT 0 0` asks `RediSearch` for the total only, so the reply is a single
/// integer however many documents match. This is the cheap counterpart to
/// [`ft_aggregate`](super::ft_aggregate::ft_aggregate), which has to `LOAD` a
/// field from every matching document and page through them with a cursor.
pub(crate) async fn ft_search_count<C>(
    mut connection_manager: C,
    index: String,
    query: String,
) -> Result<u64, RedisError>
where
    C: ConnectionLike + Send,
{
    let res = redis::cmd("FT.SEARCH")
        .arg(&index)
        .arg(&query)
        .arg("LIMIT")
        .arg(0)
        .arg(0)
        .arg("TIMEOUT")
        .arg(FT_SEARCH_TIMEOUT_MS)
        .query_async::<Value>(&mut connection_manager)
        .await;
    let value = match res {
        Ok(value) => value,
        Err(e) => {
            error!(?e, index, ?query, "Error calling ft.search");
            return Err(e);
        }
    };
    parse_total(&value)
}

/// Reads the match total out of an `FT.SEARCH` reply.
///
/// RESP2 answers with an array whose first element is the total; RESP3 answers
/// with a map holding `total_results`.
fn parse_total(value: &Value) -> Result<u64, RedisError> {
    let total = match value {
        Value::Array(items) => match items.first() {
            Some(Value::Int(total)) => *total,
            other => {
                error!(?other, "Non-int for first value in ft.search");
                return Err(RedisError::from((
                    ErrorKind::Parse,
                    "Non int for search total",
                    format!("{other:?}"),
                )));
            }
        },
        Value::Map(entries) => {
            let total = entries.iter().find_map(|(key, value)| match (key, value) {
                (Value::SimpleString(key), Value::Int(total)) if key == "total_results" => {
                    Some(*total)
                }
                _ => None,
            });
            let Some(total) = total else {
                error!(?entries, "No total_results in ft.search reply");
                return Err(RedisError::from((
                    ErrorKind::Parse,
                    "No total_results in ft.search reply",
                    format!("{entries:?}"),
                )));
            };
            total
        }
        other => {
            error!(?other, "Unexpected top-level value in ft.search reply");
            return Err(RedisError::from((
                ErrorKind::Parse,
                "Expected array or map",
                format!("{other:?}"),
            )));
        }
    };
    u64::try_from(total).map_err(|_| {
        RedisError::from((
            ErrorKind::Parse,
            "Negative total in ft.search reply",
            format!("{total}"),
        ))
    })
}

#[cfg(test)]
mod tests {
    use redis::Value;

    use super::parse_total;

    #[test]
    fn parses_resp2_total() {
        let value = Value::Array(vec![Value::Int(7)]);
        assert_eq!(parse_total(&value).unwrap(), 7);
    }

    #[test]
    fn parses_resp3_total() {
        let value = Value::Map(vec![
            (
                Value::SimpleString("attributes".to_string()),
                Value::Array(vec![]),
            ),
            (
                Value::SimpleString("total_results".to_string()),
                Value::Int(3),
            ),
        ]);
        assert_eq!(parse_total(&value).unwrap(), 3);
    }

    #[test]
    fn rejects_missing_total() {
        let value = Value::Map(vec![(
            Value::SimpleString("attributes".to_string()),
            Value::Array(vec![]),
        )]);
        assert!(parse_total(&value).is_err());
    }

    #[test]
    fn rejects_negative_total() {
        let value = Value::Array(vec![Value::Int(-1)]);
        assert!(parse_total(&value).is_err());
    }
}
