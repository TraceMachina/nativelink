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

/// Only a zombie's parent is reported; a live process, whatever its parent,
/// is nobody's to reap.
#[test]
fn parse_zombie_ppid_reads_only_zombies() {
    let zombie = "Name:\tsleep\nState:\tZ (zombie)\nTgid:\t42\nPid:\t42\nPPid:\t7\n";
    assert_eq!(parse_zombie_ppid(zombie), Some(7));
    let live = "Name:\tsleep\nState:\tS (sleeping)\nPid:\t42\nPPid:\t7\n";
    assert_eq!(parse_zombie_ppid(live), None);
    assert_eq!(parse_zombie_ppid("State:\tZ (zombie)\n"), None);
    assert_eq!(parse_zombie_ppid(""), None);
}

/// A process an action forked and did not wait for is reparented to the
/// worker and becomes a zombie when it exits. It survives one sweep, which
/// only notes it, and is reaped by the next; a zombie that lasts an
/// interval is nothing the runtime is about to reap itself.
#[cfg(target_os = "linux")]
#[nativelink_macro::nativelink_test]
async fn an_orphaned_zombie_is_reaped_on_the_second_sweep() -> Result<(), nativelink_error::Error> {
    use std::collections::HashSet;

    use nativelink_worker::reaper::{become_subreaper, reap_orphaned_zombies, zombie_children};

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
    tokio::time::sleep(core::time::Duration::from_millis(600)).await;
    assert!(
        zombie_children().contains(&orphan),
        "the orphan should be our zombie by now"
    );

    let mut seen_last = HashSet::new();
    assert_eq!(
        reap_orphaned_zombies(&mut seen_last),
        0,
        "the first sweep only notes it"
    );
    assert!(seen_last.contains(&orphan));
    assert_eq!(
        reap_orphaned_zombies(&mut seen_last),
        1,
        "the second sweep reaps it"
    );
    assert!(
        !std::path::Path::new(&format!("/proc/{orphan}")).exists(),
        "the zombie should be gone"
    );
    Ok(())
}
