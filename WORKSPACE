# Copyright 2020-2021 Nathan (Blaise) Bruer.  All rights reserved.

workspace(name = "rust_cas")

load("@bazel_tools//tools/build_defs/repo:http.bzl", "http_archive")

http_archive(
    name = "rules_rust",
    sha256 = "accb5a89cbe63d55dcdae85938e56ff3aa56f21eb847ed826a28a83db8500ae6",
    strip_prefix = "rules_rust-9aa49569b2b0dacecc51c05cee52708b7255bd98",
    urls = [
        # Main branch as of 2021-02-19
        "https://github.com/bazelbuild/rules_rust/archive/9aa49569b2b0dacecc51c05cee52708b7255bd98.tar.gz",
    ],
)

load("@rules_rust//rust:repositories.bzl", "rust_repositories")
rust_repositories(version = "nightly", iso_date = "2020-11-10", edition="2018")

load("//third_party:crates.bzl", "raze_fetch_remote_crates")
raze_fetch_remote_crates()

load("@rules_rust//proto:repositories.bzl", "rust_proto_repositories")
rust_proto_repositories()
