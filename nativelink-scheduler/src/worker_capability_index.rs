// Copyright 2024 The NativeLink Authors. All rights reserved.
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

//! Worker capability index for fast worker matching.
//!
//! This module provides an index that accelerates worker matching by property.
//! Instead of iterating all workers for each action, we maintain an inverted index
//! that maps property values to sets of workers that have those values.
//!
//! ## Complexity Analysis
//!
//! Without index: O(W × P) where W = workers, P = properties per action
//! With index: O(P × log(W)) for exact properties + O(W' × P') for minimum properties
//!   where W' = filtered workers, P' = minimum property count (typically small)
//!
//! For typical workloads (few minimum properties), this reduces matching from
//! O(n × m) to approximately O(log n).

use std::collections::{HashMap, HashSet};

use nativelink_util::action_messages::WorkerId;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use tracing::info;

/// A property key-value pair used for indexing.
#[derive(Clone, Hash, Eq, PartialEq, Debug)]
struct PropertyKey {
    name: String,
    value: PlatformPropertyValue,
}

/// Index structure for fast worker capability lookup.
///
/// Maintains an inverted index from property values to worker IDs.
/// Only indexes `Exact` and `Priority` properties since `Minimum` properties
/// are dynamic and require runtime comparison.
#[derive(Debug, Default)]
pub struct WorkerCapabilityIndex {
    /// Maps `(property_name, property_value)` -> Set of worker IDs with that property.
    /// Only contains `Exact` and `Priority` properties.
    exact_index: HashMap<PropertyKey, HashSet<WorkerId>>,

    /// Maps `property_name` -> Set of worker IDs that have this property (any value).
    /// Used for fast "has property" checks for `Priority` and `Minimum` properties.
    property_presence: HashMap<String, HashSet<WorkerId>>,

    /// Set of all indexed worker IDs.
    all_workers: HashSet<WorkerId>,
}

impl WorkerCapabilityIndex {
    /// Creates a new empty capability index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a worker to the index with their platform properties.
    pub fn add_worker(&mut self, worker_id: &WorkerId, properties: &PlatformProperties) {
        self.all_workers.insert(worker_id.clone());

        for (name, value) in &properties.properties {
            // Track property presence
            self.property_presence
                .entry(name.clone())
                .or_default()
                .insert(worker_id.clone());

            match value {
                PlatformPropertyValue::Exact(_)
                | PlatformPropertyValue::Priority(_)
                | PlatformPropertyValue::Unknown(_) => {
                    // Index exact-match properties
                    let key = PropertyKey {
                        name: name.clone(),
                        value: value.clone(),
                    };
                    self.exact_index
                        .entry(key)
                        .or_default()
                        .insert(worker_id.clone());
                }
                PlatformPropertyValue::Minimum(_) => {
                    // Minimum properties are tracked via property_presence only.
                    // Their actual values are checked at runtime since they're dynamic.
                }
            }
        }
    }

    /// Removes a worker from the index.
    pub fn remove_worker(&mut self, worker_id: &WorkerId) {
        self.all_workers.remove(worker_id);

        // Remove from exact index
        self.exact_index.retain(|_, workers| {
            workers.remove(worker_id);
            !workers.is_empty()
        });

        // Remove from presence index
        self.property_presence.retain(|_, workers| {
            workers.remove(worker_id);
            !workers.is_empty()
        });
    }

    /// Finds workers that can satisfy the given action properties.
    ///
    /// Returns a set of worker IDs that match all required properties.
    /// The caller should apply additional filtering (e.g., worker availability).
    ///
    /// IMPORTANT: This method returns candidates based on STATIC properties only.
    /// - Exact and Unknown properties are fully matched
    /// - Priority properties just require the key to exist
    /// - Minimum properties return workers that HAVE the property (presence check only)
    ///
    /// The caller MUST still verify Minimum property values at runtime because
    /// worker resources change dynamically as jobs are assigned/completed.
    pub fn find_matching_workers(
        &self,
        action_properties: &PlatformProperties,
        full_worker_logging: bool,
    ) -> HashSet<WorkerId> {
        if self.all_workers.is_empty() {
            if full_worker_logging {
                info!("No workers available to match!");
            }
            return HashSet::new();
        }

        if action_properties.properties.is_empty() {
            // No properties required, all workers match
            return self.all_workers.clone();
        }

        let mut candidates: Option<HashSet<WorkerId>> = None;

        for (name, value) in &action_properties.properties {
            match value {
                PlatformPropertyValue::Exact(_) | PlatformPropertyValue::Unknown(_) => {
                    // Look up workers with exact match
                    let key = PropertyKey {
                        name: name.clone(),
                        value: value.clone(),
                    };

                    let matching = self.exact_index.get(&key).cloned().unwrap_or_default();

                    let internal_candidates = match candidates {
                        Some(existing) => existing.intersection(&matching).cloned().collect(),
                        None => matching,
                    };

                    // Early exit if no candidates
                    if internal_candidates.is_empty() {
                        if full_worker_logging {
                            info!(
                                "No candidate workers due to a lack of matching '{name}' = {value:?}"
                            );
                        }
                        return HashSet::new();
                    }
                    candidates = Some(internal_candidates);
                }
                PlatformPropertyValue::Priority(_) | PlatformPropertyValue::Minimum(_) => {
                    // Priority: just requires the key to exist
                    // Minimum: worker must have the property (value checked at runtime by caller)
                    // We only check presence here because Minimum values are DYNAMIC -
                    // they change as jobs are assigned to workers.
                    let workers_with_property = self
                        .property_presence
                        .get(name)
                        .cloned()
                        .unwrap_or_default();

                    let internal_candidates = match candidates {
                        Some(existing) => existing
                            .intersection(&workers_with_property)
                            .cloned()
                            .collect(),
                        None => workers_with_property,
                    };

                    if internal_candidates.is_empty() {
                        if full_worker_logging {
                            info!(
                                "No candidate workers due to a lack of key '{name}'. Job asked for {value:?}"
                            );
                        }
                        return HashSet::new();
                    }
                    candidates = Some(internal_candidates);
                }
            }
        }

        candidates.unwrap_or_else(|| self.all_workers.clone())
    }

    /// Returns the number of indexed workers.
    pub fn worker_count(&self) -> usize {
        self.all_workers.len()
    }

    /// Returns true if the index is empty.
    pub fn is_empty(&self) -> bool {
        self.all_workers.is_empty()
    }
}
