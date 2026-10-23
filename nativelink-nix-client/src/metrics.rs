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

//! OpenTelemetry metrics, emitted over OTLP exactly as the NativeLink server
//! does (via [`nativelink_util::telemetry`] and the global meter provider), so
//! a collector's ClickHouse exporter reads client/daemon metrics with no extra
//! wiring. Point `NL_OTEL_ENDPOINT` at the collector.

use opentelemetry::global;
use opentelemetry::metrics::{Counter, Histogram};

/// Push/pull counters and latency histograms for one client or daemon.
///
/// Cheap to clone-by-`Arc`; every instrument is backed by the process-global
/// meter provider that [`nativelink_util::telemetry::init_tracing`] installs.
#[derive(Debug)]
pub struct Metrics {
    paths_pushed: Counter<u64>,
    paths_deduped: Counter<u64>,
    push_errors: Counter<u64>,
    nar_bytes: Counter<u64>,
    wire_bytes: Counter<u64>,
    push_seconds: Histogram<f64>,
    paths_pulled: Counter<u64>,
    pull_errors: Counter<u64>,
    pulled_bytes: Counter<u64>,
}

impl Metrics {
    /// Builds the instrument set under the meter named `component`
    /// (e.g. `"nl-nix"` or `"nl-watch-store"`).
    #[must_use]
    pub fn new(component: &'static str) -> Self {
        let m = global::meter(component);
        Self {
            paths_pushed: m
                .u64_counter("nl.nix.paths_pushed")
                .with_description("Store paths uploaded to the cache")
                .build(),
            paths_deduped: m
                .u64_counter("nl.nix.paths_deduped")
                .with_description("Paths skipped because the cache already had them")
                .build(),
            push_errors: m
                .u64_counter("nl.nix.push_errors")
                .with_description("Failed path pushes")
                .build(),
            nar_bytes: m
                .u64_counter("nl.nix.nar_bytes")
                .with_unit("By")
                .with_description("Uncompressed NAR bytes pushed")
                .build(),
            wire_bytes: m
                .u64_counter("nl.nix.wire_bytes")
                .with_unit("By")
                .with_description("Compressed bytes actually sent on the wire")
                .build(),
            push_seconds: m
                .f64_histogram("nl.nix.push_seconds")
                .with_unit("s")
                .with_description("Wall-clock seconds per path push")
                .build(),
            paths_pulled: m
                .u64_counter("nl.nix.paths_pulled")
                .with_description("Store paths downloaded from the cache")
                .build(),
            pull_errors: m
                .u64_counter("nl.nix.pull_errors")
                .with_description("Failed path pulls")
                .build(),
            pulled_bytes: m
                .u64_counter("nl.nix.pulled_bytes")
                .with_unit("By")
                .with_description("Uncompressed NAR bytes pulled")
                .build(),
        }
    }

    /// Records a successful push.
    pub fn push_ok(&self, nar_size: u64, wire_bytes: u64, seconds: f64) {
        self.paths_pushed.add(1, &[]);
        self.nar_bytes.add(nar_size, &[]);
        self.wire_bytes.add(wire_bytes, &[]);
        self.push_seconds.record(seconds, &[]);
    }

    /// Records a push skipped by dedup.
    pub fn push_deduped(&self) {
        self.paths_deduped.add(1, &[]);
    }

    /// Records a failed push.
    pub fn push_error(&self) {
        self.push_errors.add(1, &[]);
    }

    /// Records a successful pull.
    pub fn pull_ok(&self, nar_size: u64) {
        self.paths_pulled.add(1, &[]);
        self.pulled_bytes.add(nar_size, &[]);
    }

    /// Records a failed pull.
    pub fn pull_error(&self) {
        self.pull_errors.add(1, &[]);
    }
}
