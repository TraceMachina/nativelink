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

use nativelink_worker::reaper::parse_zombie_ppid;

/// Only a zombie's parent is reported, read after the final `)` so a comm
/// with spaces or parentheses cannot shift the fields; a live process,
/// whatever its parent, is nobody's to reap.
#[test]
fn parse_zombie_ppid_reads_only_zombies() {
    assert_eq!(
        parse_zombie_ppid("42 (sleep) Z 7 42 42 0 -1 4194560 0\n"),
        Some(7)
    );
    assert_eq!(parse_zombie_ppid("42 (a b) c) Z 7 42 42 0\n"), Some(7));
    assert_eq!(parse_zombie_ppid("42 (sleep) S 7 42 42 0\n"), None);
    assert_eq!(parse_zombie_ppid("42 (sleep) Z\n"), None);
    assert_eq!(parse_zombie_ppid(""), None);
}

/// A process an action forked and did not wait for is reparented to the
/// worker and becomes a zombie when it exits. It survives one sweep, which
/// only notes it, and is reaped by the next; a child this process spawned
/// and still holds is never reaped, however long it sits, since its exit
/// status belongs to the handle.
#[cfg(target_os = "linux")]
#[nativelink_macro::nativelink_test]
async fn an_orphaned_zombie_is_reaped_on_the_second_sweep_and_an_owned_one_never()
-> Result<(), nativelink_error::Error> {
    use std::collections::BTreeSet;

    use nativelink_worker::reaper::{
        OwnedChild, become_subreaper, reap_orphaned_zombies, zombie_children,
    };

    become_subreaper()?;
    // The shell backgrounds a short sleep, prints its pid and exits, which
    // orphans the sleep onto us; the sleep then exits and waits for a reaper.
    let output = tokio::process::Command::new("sh")
        .args(["-c", "sleep 0.2 & echo $!"])
        .output()
        .await?;
    let orphan: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap();
    // A child of our own that has exited and that nobody has waited on yet:
    // a persistent worker between requests looks like this.
    let mut ours = tokio::process::Command::new("true").spawn()?;
    let ours_pid = ours.id().expect("just spawned");
    let owned = OwnedChild::new(Some(ours_pid));
    tokio::time::sleep(core::time::Duration::from_millis(600)).await;
    let zombies = zombie_children();
    assert!(
        zombies.contains(&orphan),
        "the orphan should be our zombie by now"
    );
    assert!(
        zombies.contains(&ours_pid),
        "our own child should be a zombie too"
    );

    let (reaped, seen) = reap_orphaned_zombies(BTreeSet::new());
    assert_eq!(reaped, 0, "the first sweep only notes them");
    assert!(seen.contains(&orphan) && seen.contains(&ours_pid));
    let (reaped, seen) = reap_orphaned_zombies(seen);
    assert_eq!(reaped, 1, "the second sweep reaps the orphan alone");
    assert!(
        !std::path::Path::new(&format!("/proc/{orphan}")).exists(),
        "the orphan should be gone"
    );
    assert!(
        !seen.contains(&orphan),
        "a reaped pid is not carried into the next sweep"
    );
    assert!(seen.contains(&ours_pid), "ours is still there, still noted");
    // The handle collects its own child, as it always could.
    let status = ours.wait().await?;
    assert!(status.success());
    drop(owned);
    Ok(())
}
