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

//! # nativelink-nix
//!
//! Nix binary-cache protocol primitives for `NativeLink`: the nixbase32
//! encoding, the `.narinfo` wire format (parsing, rendering, and the
//! signing fingerprint), and Nix-compatible ed25519 key handling.
//!
//! These are the pure building blocks for the Nix substituter facade — an
//! HTTP service that serves the Nix binary-cache protocol backed by
//! `NativeLink` stores, mirroring how `nativelink-oci` bridges OCI content
//! into the CAS.

pub mod nar_url;
pub mod narinfo;
pub mod nixbase32;
pub mod path_info;
pub mod signing;
