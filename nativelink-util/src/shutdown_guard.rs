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

use std::collections::HashMap;

use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Priority {
    // The least important priority.
    LeastImportant = 2,
    // All priorities greater than 1 must be completed.
    P1 = 1,
    // All other priorities must be completed.
    P0 = 0,
}

impl From<usize> for Priority {
    fn from(value: usize) -> Self {
        match value {
            0 => Self::P0,
            1 => Self::P1,
            _ => Self::LeastImportant,
        }
    }
}

impl From<Priority> for usize {
    fn from(priority: Priority) -> Self {
        match priority {
            Priority::P0 => 0,
            Priority::P1 => 1,
            Priority::LeastImportant => 2,
        }
    }
}

/// Tracks other services that have registered to be notified when
/// the process is being shut down.
#[derive(Debug)]
pub struct ShutdownGuard {
    priority: Priority,
    tx: watch::Sender<HashMap<Priority, usize>>,
    rx: watch::Receiver<HashMap<Priority, usize>>,
}

impl ShutdownGuard {
    /// Waits for all priorities less important than the given
    /// priority to be completed.
    pub async fn wait_for(&mut self, priority: Priority) {
        if priority != self.priority {
            // Promote our priority to the new priority.
            self.tx.send_modify(|map| {
                let old_count = map.remove(&self.priority).unwrap_or(0).saturating_sub(1);
                map.insert(self.priority, old_count);

                self.priority = priority;

                let new_count = map.get(&priority).unwrap_or(&0).saturating_add(1);
                map.insert(priority, new_count);
            });
        }
        // Ignore error because the receiver will never be closed
        // if the sender is still alive here.
        drop(
            self.rx
                .wait_for(|map| {
                    let start = usize::from(priority) + 1;
                    let end = usize::from(Priority::LeastImportant);
                    for p in start..=end {
                        if *map.get(&p.into()).unwrap_or(&0) > 0 {
                            return false;
                        }
                    }
                    true
                })
                .await,
        );
    }
}

impl Default for ShutdownGuard {
    fn default() -> Self {
        let priority = Priority::LeastImportant;
        let mut map = HashMap::new();
        map.insert(priority, 0);
        let (tx, rx) = watch::channel(map);
        Self { priority, tx, rx }
    }
}

impl Clone for ShutdownGuard {
    fn clone(&self) -> Self {
        self.tx.send_modify(|map| {
            map.insert(
                self.priority,
                map.get(&Priority::LeastImportant)
                    .unwrap_or(&0)
                    .saturating_add(1),
            );
        });
        Self {
            priority: Priority::LeastImportant,
            tx: self.tx.clone(),
            rx: self.rx.clone(),
        }
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        self.tx.send_modify(|map| {
            map.insert(
                self.priority,
                map.get(&self.priority).unwrap_or(&0).saturating_sub(1),
            );
        });
    }
}
