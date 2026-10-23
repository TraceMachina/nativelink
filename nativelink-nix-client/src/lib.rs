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

//! A Nix binary-cache client and watch-store daemon, co-designed with the
//! NativeLink `nix_cache` service.
//!
//! Two executables share the core in this crate:
//!
//! - **`nl-nix`** — a `nix copy`-style client that pushes and pulls store
//!   paths over the Nix HTTP binary-cache protocol. It serializes each store
//!   path to a NAR in pure Rust ([`nar`]), streams it zstd-compressed on the
//!   wire (co-designed with the server's `serve_compression` /
//!   `preserve_upload_compression`), and reuses [`nativelink_nix`] for the
//!   `narinfo` metadata, canonical NAR names, and Ed25519 signing.
//! - **`nl-watch-store`** — a daemon that watches the local store with
//!   `fanotify` ([`watch`]) and auto-pushes every newly-committed path,
//!   deduplicating against the cache.
//!
//! Both emit OpenTelemetry metrics ([`metrics`]) over OTLP, exactly as the
//! NativeLink server does, so a collector's ClickHouse exporter can read them.
//!
//! The protocol primitives (narinfo parse/render, the signature fingerprint,
//! nixbase32, canonical `.nar`/`.nar.zst` names) live in [`nativelink_nix`] and
//! are shared with the server — the client and server are two ends of one wire
//! format.

pub mod client;
pub mod config;
pub mod metrics;
pub mod nar;
pub mod store;
pub mod watch;

pub use config::{Auth, CacheConfig};
pub use nar::{HashingNar, dump_path, restore_path};
pub use store::{NixStore, PathMeta, StorePath};
