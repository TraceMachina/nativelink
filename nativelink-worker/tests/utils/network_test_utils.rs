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

//! Helpers for the tests of `isolate_network`.

use std::collections::BTreeSet;

/// The interface names in a copy of `/proc/net/dev`, which lists the
/// network namespace of whoever reads it.
pub(crate) fn interface_names(proc_net_dev: &str) -> BTreeSet<String> {
    proc_net_dev
        .lines()
        .skip(2)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, _)| name.trim().to_string())
        .collect()
}

/// What an isolated process sees: loopback and nothing else.
pub(crate) fn only_loopback() -> BTreeSet<String> {
    BTreeSet::from(["lo".to_string()])
}

/// The interfaces of this process's network namespace, or None, said on
/// stderr, when it has nothing beyond loopback: there an isolated process
/// and one that is not look the same, so `test` cannot tell them apart.
pub(crate) fn host_interfaces_beyond_loopback(test: &str) -> Option<BTreeSet<String>> {
    let interfaces = interface_names(&std::fs::read_to_string("/proc/net/dev").ok()?);
    if interfaces == only_loopback() {
        skip(test, "this host has no network interface beyond loopback");
        return None;
    }
    Some(interfaces)
}

/// Says on stderr that `test` checked nothing, and why.
pub(crate) fn skip(test: &str, why: &str) {
    eprintln!("{test}: skipped, {why}");
}
