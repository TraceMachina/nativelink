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

//! The result of trying to place one queued action on the worker fleet.

use core::fmt;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use nativelink_util::action_messages::WorkerId;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};

/// How many distinct worker values are named for one unsatisfied property.
const MAX_OFFERED_VALUES: usize = 8;

/// What happened when the scheduler looked for a worker for an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchOutcome {
    /// This worker can run the action now.
    Matched(WorkerId),
    /// A worker could run the action when idle, but none has room right now.
    WaitingForCapacity,
    /// No workers are connected at all.
    NoWorkersConnected,
    /// No worker could run the action even when fully idle.
    Unsatisfiable(Arc<UnsatisfiableReason>),
}

/// One action property that the fleet cannot satisfy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsatisfiedProperty {
    pub name: String,
    pub requested: PlatformPropertyValue,
    /// For a `Minimum` property, the largest total any worker registered
    /// with. Otherwise the distinct values workers registered with, capped at
    /// `MAX_OFFERED_VALUES`. Empty when no worker declares the property.
    pub offered: Vec<String>,
    /// How many distinct values were left out of `offered`.
    pub offered_omitted: usize,
}

/// Why no worker can run an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsatisfiableReason {
    /// Sorted by property name.
    pub properties: Vec<UnsatisfiedProperty>,
    /// True when every property is satisfied by some worker, but no single
    /// worker satisfies all of them. `properties` then lists every property
    /// that restricts matching.
    pub combination_only: bool,
}

impl UnsatisfiableReason {
    /// The unsatisfied property names, comma separated, for metric labels.
    #[must_use]
    pub fn property_names(&self) -> String {
        self.properties
            .iter()
            .map(|property| property.name.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl fmt::Display for UnsatisfiableReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.combination_only {
            f.write_str("no single worker satisfies the combination of: ")?;
        } else {
            f.write_str("no worker can satisfy: ")?;
        }
        for (i, property) in self.properties.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            // A priority property only needs the key on the worker, so its
            // value is not what went unsatisfied.
            if matches!(property.requested, PlatformPropertyValue::Priority(_)) {
                write!(f, "'{}' is required", property.name)?;
            } else {
                write!(
                    f,
                    "'{}' requested {}",
                    property.name,
                    property.requested.as_str()
                )?;
            }
            if property.offered.is_empty() {
                f.write_str(", no worker declares this property")?;
            } else if matches!(property.requested, PlatformPropertyValue::Minimum(_)) {
                write!(f, ", largest worker total {}", property.offered[0])?;
            } else {
                write!(f, ", workers offer [{}]", property.offered.join(", "))?;
                if property.offered_omitted > 0 {
                    write!(f, " and {} more", property.offered_omitted)?;
                }
            }
        }
        Ok(())
    }
}

/// The parts of an action's platform properties that decide which workers can
/// run it, in a form that can key a map. `Ignore` properties never restrict
/// matching and a `Priority` property only requires the key, so actions that
/// differ only in those share a shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PropertyShape(Vec<(String, PlatformPropertyValue)>);

impl From<&PlatformProperties> for PropertyShape {
    fn from(platform_properties: &PlatformProperties) -> Self {
        let mut shape: Vec<_> = platform_properties
            .properties
            .iter()
            .filter_map(|(name, value)| match value {
                PlatformPropertyValue::Ignore(_) => None,
                PlatformPropertyValue::Priority(_) => {
                    Some((name.clone(), PlatformPropertyValue::Priority(String::new())))
                }
                _ => Some((name.clone(), value.clone())),
            })
            .collect();
        shape.sort_unstable();
        Self(shape)
    }
}

fn offered_values(
    name: &str,
    requested: &PlatformPropertyValue,
    fleet: &[&PlatformProperties],
) -> (Vec<String>, usize) {
    let declared = fleet
        .iter()
        .filter_map(|worker_totals| worker_totals.properties.get(name));
    if matches!(requested, PlatformPropertyValue::Minimum(_)) {
        let largest = declared
            .filter_map(|value| match value {
                PlatformPropertyValue::Minimum(total) => Some(*total),
                _ => None,
            })
            .max();
        return (largest.map(|v| v.to_string()).into_iter().collect(), 0);
    }
    let distinct: BTreeSet<String> = declared.map(|value| value.as_str().into_owned()).collect();
    let omitted = distinct.len().saturating_sub(MAX_OFFERED_VALUES);
    (
        distinct.into_iter().take(MAX_OFFERED_VALUES).collect(),
        omitted,
    )
}

/// Works out which properties stop every worker in `fleet` from running the
/// action. `fleet` holds the properties each worker registered with. Only
/// call this once it is known that no worker in `fleet` satisfies the action.
#[must_use]
pub fn explain_unsatisfiable(
    action_properties: &PlatformProperties,
    fleet: &[&PlatformProperties],
) -> UnsatisfiableReason {
    let mut restricting: Vec<_> = action_properties
        .properties
        .iter()
        .filter(|(_, value)| !matches!(value, PlatformPropertyValue::Ignore(_)))
        .collect();
    restricting.sort_unstable_by(|a, b| a.0.cmp(b.0));

    let to_unsatisfied = |(name, requested): &(&String, &PlatformPropertyValue)| {
        let (offered, offered_omitted) = offered_values(name, requested, fleet);
        UnsatisfiedProperty {
            name: (*name).clone(),
            requested: (*requested).clone(),
            offered,
            offered_omitted,
        }
    };

    // How many workers each property rules out on its own.
    let rules_out = |(name, requested): &(&String, &PlatformPropertyValue)| -> usize {
        let alone =
            PlatformProperties::new(HashMap::from([((*name).clone(), (*requested).clone())]));
        fleet
            .iter()
            .filter(|worker_totals| !alone.is_satisfied_by(worker_totals, false))
            .count()
    };

    let properties: Vec<_> = restricting
        .iter()
        .filter(|property| rules_out(property) == fleet.len())
        .map(to_unsatisfied)
        .collect();

    if properties.is_empty() {
        // Only the properties that rule out some worker take part in the
        // combination; one every worker satisfies is not a cause.
        return UnsatisfiableReason {
            properties: restricting
                .iter()
                .filter(|property| rules_out(property) > 0)
                .map(to_unsatisfied)
                .collect(),
            combination_only: true,
        };
    }
    UnsatisfiableReason {
        properties,
        combination_only: false,
    }
}
