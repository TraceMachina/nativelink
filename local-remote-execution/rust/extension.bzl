# Copyright 2024 The NativeLink Authors. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# This file is @generated by lre-rs.

"""Module extension to register different lre-rs tool repositories."""

load("@bazel_tools//tools/build_defs/repo:local.bzl", "new_local_repository")

# TODO(aaronmondal): Using module extensions here isn't optimal as it doesn't
#                    allow overriding tools via environment variables.
#                    Unfortunately rules_rust's rust_toolchain currently
#                    requires tools to be declared via labels to filegroups,
#                    so we need the new_local_repository approach here.
#                    Add support for raw strings to rules_rust upstream so
#                    that we can remove this module extension entirely.

def _lre_rs_impl(_mctx):
    new_local_repository(
        name = "lre-rs-stable-aarch64-darwin",
        build_file = "@local-remote-execution//rust:aarch64-darwin.BUILD.bazel",
        path = "/nix/store/g0c2pzqgm6hfd87f8s76iwnq93x9pyk2-rust-default-1.84.0",
    )
    new_local_repository(
        name = "lre-rs-nightly-aarch64-darwin",
        build_file = "@local-remote-execution//rust:aarch64-darwin.BUILD.bazel",
        path = "/nix/store/6avh6lc3lr91kk4h0lz30q55drbllg7n-rust-default-1.85.0-nightly-2024-11-23",
    )
    new_local_repository(
        name = "lre-rs-stable-aarch64-linux",
        build_file = "@local-remote-execution//rust:aarch64-linux.BUILD.bazel",
        path = "/nix/store/jqsg3b5al7d43823xj6mm7fmilqd17rp-rust-default-1.84.0",
    )
    new_local_repository(
        name = "lre-rs-nightly-aarch64-linux",
        build_file = "@local-remote-execution//rust:aarch64-linux.BUILD.bazel",
        path = "/nix/store/80b3pgc22zvvqldjc8acjdi8k9v3i5bm-rust-default-1.85.0-nightly-2024-11-23",
    )
    new_local_repository(
        name = "lre-rs-stable-x86_64-darwin",
        build_file = "@local-remote-execution//rust:x86_64-darwin.BUILD.bazel",
        path = "/nix/store/9irgcl09m4skp8n9wraxz6cc9bv4xqvj-rust-default-1.84.0",
    )
    new_local_repository(
        name = "lre-rs-nightly-x86_64-darwin",
        build_file = "@local-remote-execution//rust:x86_64-darwin.BUILD.bazel",
        path = "/nix/store/2mji7ysl9yjjg7szlfi4vm6bldcbkv4a-rust-default-1.85.0-nightly-2024-11-23",
    )
    new_local_repository(
        name = "lre-rs-stable-x86_64-linux",
        build_file = "@local-remote-execution//rust:x86_64-linux.BUILD.bazel",
        path = "/nix/store/y66fvwa0qghjk9wr5f9b843lmcsqp581-rust-default-1.84.0",
    )
    new_local_repository(
        name = "lre-rs-nightly-x86_64-linux",
        build_file = "@local-remote-execution//rust:x86_64-linux.BUILD.bazel",
        path = "/nix/store/v1sh2gqjz73pqp7qaa513fvp89fmd8yg-rust-default-1.85.0-nightly-2024-11-23",
    )

lre_rs = module_extension(implementation = _lre_rs_impl)
