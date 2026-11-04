// Copyright 2026 Trace Machina, Inc. All rights reserved.
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
//! 3. once the new one has every key, reads move to it. "Every key" is
//!    checked, not assumed from the scan having ended: a scan that runs out
//!    of memory stops with keys missing, and dropping the old index then
//!    would leave reads on an index without them;
//! 4. the old one is dropped with `_FORCEKEEPDOCS`, by whichever process
//!    gets there first.
//!
//! Every step is idempotent: a create that loses a race finds the index
//! there, a drop that loses one finds it gone. A process that stops
//! anywhere leaves a state the next read carries on from.

use redis::aio::ConnectionLike;
use redis::{RedisError, Value};

use super::FtInfo;

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

/// Redis refused a write because it is at `maxmemory` under `noeviction`.
/// Building the replacement is a write, so a full Redis cannot start the
/// migration; dropping an index is still allowed there.
pub(crate) fn is_out_of_memory(err: &RedisError) -> bool {
    error_text(err).contains("oom command not allowed")
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

/// Where the persistent replacement stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Replacement {
    /// The background scan is still running.
    Backfilling,
    /// The scan finished and the index holds as many keys as the original.
    Complete,
    /// The scan finished without every key: some failed to index, or it
    /// holds fewer keys than the original.
    Incomplete,
}

/// Judges a replacement that `FT.INFO` describes as `info` against the
/// original's key count (`None` when there is no original to compare with).
///
/// The comparison is strict: both indexes cover the same key prefix and both
/// take live writes, so at any one instant a complete replacement holds at
/// least every key the original does. The caller reads the two counts in one
/// `MULTI`/`EXEC` ([`super::ft_info_pair`]) so they are from the same
/// instant, which is what makes strictness correct — no tolerance for racing
/// deletes, and therefore no tolerance a short scan could hide inside. Any
/// shortfall, and any reported indexing failure, keeps the original.
pub(crate) const fn replacement_state(info: FtInfo, original_docs: Option<u64>) -> Replacement {
    if info.indexing {
        return Replacement::Backfilling;
    }
    if matches!(info.hash_indexing_failures, Some(failed) if failed != 0) {
        return Replacement::Incomplete;
    }
    match original_docs {
        Some(original) if info.num_docs < original => Replacement::Incomplete,
        _ => Replacement::Complete,
    }
}

/// The server ended an open cursor because the index under it went away —
/// what a reader still paging the original sees when another process
/// finishes the migration and retires it (`FT.DROPINDEX` closes the index's
/// cursors even with `_FORCEKEEPDOCS`), and what a long-idle cursor sees
/// after the server reaps it. Both mean the rows read so far are fine and
/// the read can start over; nothing else about the error is known.
pub(crate) fn is_cursor_lost(err: &RedisError) -> bool {
    let text = error_text(err);
    text.contains("dropped") || text.contains("unknown index") || text.contains("cursor")
}

/// What a read should do, from the state of the original index and of its
/// persistent replacement (`None` when the replacement does not exist).
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
    /// The replacement finished without every key: keep reading the
    /// original, which still has them, and leave both in place. Dropping
    /// the replacement (`_FORCEKEEPDOCS`) makes the next read build it again.
    KeepOriginal,
}

pub(crate) const fn plan(original: IndexKind, replacement: Option<Replacement>) -> Step {
    match (original, replacement) {
        // Nothing to migrate. A missing original is created persistent by
        // the ordinary path, under its ordinary name.
        (IndexKind::Persistent | IndexKind::Missing, None) => Step::UseOriginal,
        (IndexKind::Temporary(_), None) => Step::CreateReplacement,
        (IndexKind::Temporary(_) | IndexKind::Persistent, Some(Replacement::Backfilling)) => {
            Step::WaitOnOriginal
        }
        (IndexKind::Temporary(_) | IndexKind::Persistent, Some(Replacement::Incomplete)) => {
            Step::KeepOriginal
        }
        (IndexKind::Missing, Some(Replacement::Backfilling)) => Step::WaitForReplacement,
        // With no original, the replacement is the only index there is,
        // whole or not.
        (IndexKind::Missing, Some(Replacement::Complete | Replacement::Incomplete)) => {
            Step::UseReplacement {
                retire_original: false,
            }
        }
        (IndexKind::Temporary(_) | IndexKind::Persistent, Some(Replacement::Complete)) => {
            Step::UseReplacement {
                retire_original: true,
            }
        }
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
    fn a_cursor_ended_under_a_reader_is_recognized() {
        // The message redis-stack-server 7.4.0-v0 sends when FT.DROPINDEX
        // closes an index's open cursors, and the two ways a cursor read
        // fails after the server already forgot index or cursor.
        for text in [
            "The index was dropped while the cursor was idle",
            "Unknown index name",
            "Cursor not found",
        ] {
            // The kind is irrelevant: the classifier reads only the text.
            let err =
                RedisError::from((redis::ErrorKind::UnexpectedReturnType, "", text.to_string()));
            assert!(is_cursor_lost(&err), "{text}");
        }
        let other = RedisError::from((
            redis::ErrorKind::UnexpectedReturnType,
            "",
            "OOM command not allowed when used memory > 'maxmemory'".to_string(),
        ));
        assert!(!is_cursor_lost(&other));
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
        assert_eq!(
            plan(temporary, Some(Replacement::Backfilling)),
            Step::WaitOnOriginal
        );
        assert_eq!(
            plan(temporary, Some(Replacement::Complete)),
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
            plan(IndexKind::Persistent, Some(Replacement::Complete)),
            Step::UseReplacement {
                retire_original: true
            }
        );
        assert_eq!(
            plan(IndexKind::Persistent, Some(Replacement::Backfilling)),
            Step::WaitOnOriginal
        );
    }

    #[test]
    fn without_an_original_the_replacement_is_waited_for() {
        assert_eq!(
            plan(IndexKind::Missing, Some(Replacement::Backfilling)),
            Step::WaitForReplacement
        );
        assert_eq!(
            plan(IndexKind::Missing, Some(Replacement::Complete)),
            Step::UseReplacement {
                retire_original: false
            }
        );
    }

    #[test]
    fn an_incomplete_replacement_never_retires_the_original() {
        let complete = FtInfo {
            indexing: false,
            num_docs: 26_150,
            hash_indexing_failures: Some(0),
        };
        assert_eq!(
            replacement_state(complete, Some(26_150)),
            Replacement::Complete
        );
        // Holding more than the original (keys written since it was judged
        // last) is complete too.
        assert_eq!(
            replacement_state(complete, Some(26_100)),
            Replacement::Complete
        );
        // The counts come from one MULTI/EXEC, so ANY shortfall is keys the
        // scan skipped, not a racing delete: one short of 26_150 (well
        // inside the 1%-plus-two tolerance an earlier version allowed, and
        // `indexing 0` with zero reported failures) must keep the original.
        let one_short = FtInfo {
            num_docs: 26_149,
            ..complete
        };
        assert_eq!(
            replacement_state(one_short, Some(26_150)),
            Replacement::Incomplete
        );
        // The same at a size where the old tolerance was dominated by its
        // flat "+2": two of four.
        let two_of_four = FtInfo {
            num_docs: 2,
            ..complete
        };
        assert_eq!(
            replacement_state(two_of_four, Some(4)),
            Replacement::Incomplete
        );
        let failed = FtInfo {
            hash_indexing_failures: Some(1),
            ..complete
        };
        assert_eq!(
            replacement_state(failed, Some(26_150)),
            Replacement::Incomplete
        );
        // A server that does not report hash_indexing_failures is judged by
        // the strict count alone, the same as one that reports zero.
        let unreported = FtInfo {
            hash_indexing_failures: None,
            ..complete
        };
        assert_eq!(
            replacement_state(unreported, Some(26_150)),
            Replacement::Complete
        );
        assert_eq!(
            replacement_state(unreported, Some(26_151)),
            Replacement::Incomplete
        );
        let scanning = FtInfo {
            indexing: true,
            num_docs: 0,
            ..complete
        };
        assert_eq!(
            replacement_state(scanning, Some(26_150)),
            Replacement::Backfilling
        );

        for original in [IndexKind::Temporary(86_399), IndexKind::Persistent] {
            assert_eq!(
                plan(original, Some(Replacement::Incomplete)),
                Step::KeepOriginal
            );
        }
        assert_eq!(
            plan(IndexKind::Missing, Some(Replacement::Incomplete)),
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
