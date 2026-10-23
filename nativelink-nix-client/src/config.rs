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

//! Client / daemon configuration, assembled from CLI flags and the environment.

use crate::store::DEFAULT_STORE_DIR;

/// How the client authenticates to the cache, mirroring what the server's
/// `read_token_files` / `write_token_files` accept.
#[derive(Clone, Debug)]
pub enum Auth {
    /// `Authorization: Bearer <token>`.
    Bearer(String),
    /// `Authorization: Basic base64(user:password)` — how stock `nix`
    /// authenticates via `netrc`.
    Basic { user: String, password: String },
}

/// The default zstd level the client compresses NAR uploads with (matches the
/// server's `DEFAULT_ZSTD_LEVEL`).
pub const DEFAULT_COMPRESSION_LEVEL: i32 = 3;

/// Everything needed to talk to one cache and read the local store.
#[derive(Clone, Debug)]
pub struct CacheConfig {
    /// Base URL of the cache, e.g. `https://cache.example.org` (or the mount
    /// prefix path a `nix_cache` instance is nested under).
    pub base_url: String,
    /// Optional bearer/basic credential.
    pub auth: Option<Auth>,
    /// Ed25519 secret-key files (`nix key generate-secret` format) used to sign
    /// pushed narinfo. Empty means push unsigned.
    pub signing_key_files: Vec<String>,
    /// Local store directory (`/nix/store`).
    pub store_dir: String,
    /// Whether to zstd-compress NAR uploads on the wire.
    pub compress: bool,
    /// zstd level for uploads when [`Self::compress`] is set.
    pub compression_level: i32,
    /// Maximum concurrent path pushes/pulls.
    pub concurrency: usize,
    /// Skip a push when the cache already has a structurally valid `.narinfo`.
    pub dedup: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            auth: None,
            signing_key_files: Vec::new(),
            store_dir: DEFAULT_STORE_DIR.to_string(),
            compress: true,
            compression_level: DEFAULT_COMPRESSION_LEVEL,
            concurrency: 8,
            dedup: true,
        }
    }
}
