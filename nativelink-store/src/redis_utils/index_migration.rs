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

//! Retiring the scheduler indexes an older release created `TEMPORARY`.
//!
//! On `RediSearch` 2.10 a temporary index is never garbage collected, so every
//! rewrite of an indexed hash leaves a dead entry that every later query
//! walks. Getting rid of one is the dangerous part: for a temporary index
//! both a plain `FT.DROPINDEX` (no `DD`) and the index's own idle expiry
//! delete every hash it covers (`DropIndexCommand` deletes when the index is
//! temporary; the expiry timer runs `FT.DROPINDEX <name> DD`). Only
//! `FT.DROPINDEX <name> _FORCEKEEPDOCS` removes the index alone.
//!
//! So the old index is never dropped first and never left to expire:
//!
//! 1. a persistent index is created beside it under [`migrated_name`];
//! 2. reads stay on the old one while the new one backfills, which also
//!    keeps the old one's idle timer from firing;
//! 3. once the new one has every key, reads move to it;
//! 4. the old one is dropped with `_FORCEKEEPDOCS`, by whichever process
//!    gets there first.
//!
//! Every step is idempotent: a create that loses a race finds the index
//! there, a drop that loses one finds it gone. A process that stops
//! anywhere leaves a state the next read carries on from.

use redis::aio::ConnectionLike;
use redis::{RedisError, Value};

/// The suffix of the persistent index that replaces a temporary one.
const MIGRATED_SUFFIX: &str = "_p";

/// The name of the persistent index that replaces `index`.
pub(crate) fn migrated_name(index: &str) -> String {
    format!("{index}{MIGRATED_SUFFIX}")
}

/// What `FT.DEBUG TTL` says about an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexKind {
    /// Temporary; the remaining idle time in seconds.
    Temporary(i64),
    Persistent,
    Missing,
}

/// `FT.DEBUG TTL` answers the remaining time for a temporary index and an
/// error otherwise. It reads the timer without resetting it, so asking does
/// not keep an index alive. A server that refuses `FT.DEBUG` is an error:
/// nothing can then be known, and nothing is changed.
pub(crate) async fn index_kind<C>(connection: &mut C, index: &str) -> Result<IndexKind, RedisError>
where
    C: ConnectionLike + Send,
{
    classify(
        redis::cmd("FT.DEBUG")
            .arg("TTL")
            .arg(index)
            .query_async::<Value>(connection)
            .await,
    )
}

pub(crate) fn classify(reply: Result<Value, RedisError>) -> Result<IndexKind, RedisError> {
    match reply {
        Ok(Value::Int(ttl)) => Ok(IndexKind::Temporary(ttl)),
        Ok(other) => Err(RedisError::from((
            redis::ErrorKind::UnexpectedReturnType,
            "FT.DEBUG TTL reply",
            format!("{other:?}"),
        ))),
        Err(err) => match classify_error(&error_text(&err)) {
            Some(kind) => Ok(kind),
            None => Err(err),
        },
    }
}

/// An error's text as the server sent it. `RediSearch` errors arrive split
/// into a code and a detail (`Unknown` / `index name`), so both are joined
/// rather than relying on how the error displays.
fn error_text(err: &RedisError) -> String {
    let mut text = err.to_string();
    for part in [err.code(), err.detail()].into_iter().flatten() {
        text.push(' ');
        text.push_str(part);
    }
    text.to_ascii_lowercase()
}

fn classify_error(text: &str) -> Option<IndexKind> {
    if text.contains("not temporary") {
        Some(IndexKind::Persistent)
    } else if text.contains("unknown index") {
        Some(IndexKind::Missing)
    } else {
        None
    }
}

/// Removes `index` and leaves every hash it covered in place, temporary or
/// not. Gone already is success.
pub(crate) async fn drop_keeping_documents<C>(
    connection: &mut C,
    index: &str,
) -> Result<(), RedisError>
where
    C: ConnectionLike + Send,
{
    match redis::cmd("FT.DROPINDEX")
        .arg(index)
        .arg("_FORCEKEEPDOCS")
        .query_async::<Value>(connection)
        .await
    {
        Ok(_) => Ok(()),
        Err(err) if classify_error(&error_text(&err)) == Some(IndexKind::Missing) => Ok(()),
        Err(err) => Err(err),
    }
}

/// What a read should do, from the state of the original index and of its
/// persistent replacement (`None` when the replacement does not exist,
/// `Some(true)` while it is still backfilling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Use the original, which is persistent: nothing to migrate.
    UseOriginal,
    /// Create the replacement, then read the original meanwhile.
    CreateReplacement,
    /// The replacement is backfilling; read the original meanwhile.
    WaitOnOriginal,
    /// The original is gone and the replacement is still backfilling: wait
    /// for it to finish rather than read a partial index.
    WaitForReplacement,
    /// The replacement is complete: read it, retiring the original first
    /// when it is still there.
    UseReplacement { retire_original: bool },
}

pub(crate) const fn plan(original: IndexKind, replacement_backfilling: Option<bool>) -> Step {
    match (original, replacement_backfilling) {
        // Nothing to migrate. A missing original is created persistent by
        // the ordinary path, under its ordinary name.
        (IndexKind::Persistent | IndexKind::Missing, None) => Step::UseOriginal,
        (IndexKind::Temporary(_), None) => Step::CreateReplacement,
        (IndexKind::Temporary(_) | IndexKind::Persistent, Some(true)) => Step::WaitOnOriginal,
        (IndexKind::Missing, Some(true)) => Step::WaitForReplacement,
        (IndexKind::Missing, Some(false)) => Step::UseReplacement {
            retire_original: false,
        },
        (IndexKind::Temporary(_) | IndexKind::Persistent, Some(false)) => Step::UseReplacement {
            retire_original: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ft_debug_ttl_replies_classify() {
        // The three replies redis-stack-server 7.4.0-v0 (RediSearch 2.10.5)
        // gives; the real-Redis test drives them end to end.
        assert_eq!(
            classify(Ok(Value::Int(86_399))).unwrap(),
            IndexKind::Temporary(86_399)
        );
        assert_eq!(
            classify_error("index is not temporary"),
            Some(IndexKind::Persistent)
        );
        assert_eq!(
            classify_error("unknown index name"),
            Some(IndexKind::Missing)
        );
        // Anything else is not knowledge, and nothing acts on it.
        assert_eq!(classify_error("err unknown command 'ft.debug'"), None);
        assert!(classify(Ok(Value::Okay)).is_err());
    }

    #[test]
    fn a_persistent_or_missing_index_is_left_to_the_ordinary_path() {
        assert_eq!(plan(IndexKind::Persistent, None), Step::UseOriginal);
        assert_eq!(plan(IndexKind::Missing, None), Step::UseOriginal);
    }

    #[test]
    fn a_temporary_index_is_read_until_its_replacement_is_complete() {
        let temporary = IndexKind::Temporary(86_399);
        assert_eq!(plan(temporary, None), Step::CreateReplacement);
        assert_eq!(plan(temporary, Some(true)), Step::WaitOnOriginal);
        assert_eq!(
            plan(temporary, Some(false)),
            Step::UseReplacement {
                retire_original: true
            }
        );
    }

    #[test]
    fn an_original_another_release_recreated_is_retired_again() {
        // A release without the migration recreates the original under its
        // old name; it is persistent then, and dropping it keeps the hashes.
        assert_eq!(
            plan(IndexKind::Persistent, Some(false)),
            Step::UseReplacement {
                retire_original: true
            }
        );
        assert_eq!(
            plan(IndexKind::Persistent, Some(true)),
            Step::WaitOnOriginal
        );
    }

    #[test]
    fn without_an_original_the_replacement_is_waited_for() {
        assert_eq!(
            plan(IndexKind::Missing, Some(true)),
            Step::WaitForReplacement
        );
        assert_eq!(
            plan(IndexKind::Missing, Some(false)),
            Step::UseReplacement {
                retire_original: false
            }
        );
    }

    #[test]
    fn the_replacement_sorts_after_the_original() {
        // Readers that pick the lexicographically last index with a prefix
        // (the worker provisioner does) move to the replacement on their own.
        let original = "aa__state_sort_key_3e762c15";
        assert!(migrated_name(original).as_str() > original);
        assert!(migrated_name(original).starts_with(original));
    }
}
