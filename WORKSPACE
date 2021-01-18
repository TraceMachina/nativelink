# Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

workspace(name = "rust_cas")

load("@bazel_tools//tools/build_defs/repo:http.bzl", "http_archive")

http_archive(
    name = "io_bazel_rules_rust",
    sha256 = "bbebfd5b292fa8567fa012b3d1056f2e51611fe8ac898860323b6b4d763a4efd",
    strip_prefix = "rules_rust-c7a408a8c58c6dde73a552d6f78f90c7ef96b513",
    urls = [
        # Master branch as of 2020-11-10
        "https://github.com/bazelbuild/rules_rust/archive/c7a408a8c58c6dde73a552d6f78f90c7ef96b513.tar.gz",
    ],
)

load("@io_bazel_rules_rust//rust:repositories.bzl", "rust_repositories")
rust_repositories(version = "nightly", iso_date = "2020-11-10", edition="2018")

load("//third_party:crates.bzl", "raze_fetch_remote_crates")
raze_fetch_remote_crates()

load("@io_bazel_rules_rust//proto:repositories.bzl", "rust_proto_repositories")
rust_proto_repositories()
