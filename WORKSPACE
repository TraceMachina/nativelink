# Copyright 2020-2021 Nathan (Blaise) Bruer.  All rights reserved.

workspace(name = "turbo-cache")

load("@bazel_tools//tools/build_defs/repo:http.bzl", "http_archive")

http_archive(
    name = "rules_rust",
    sha256 = "afaa4bc46bfe085252fb86f1c0a500ac811bb81831231783263558a18ba74b27",
    strip_prefix = "rules_rust-3cc41db3bea0e5b153cb49b35ec8612657e796da",
    urls = [
        # Main branch as of 2021-11-03.
        "https://github.com/bazelbuild/rules_rust/archive/3cc41db3bea0e5b153cb49b35ec8612657e796da.tar.gz",
    ],
)

load("@bazel_tools//tools/build_defs/repo:http.bzl", "http_archive")
http_archive(
    name = "rules_python",
    url = "https://github.com/bazelbuild/rules_python/releases/download/0.5.0/rules_python-0.5.0.tar.gz",
    sha256 = "cd6730ed53a002c56ce4e2f396ba3b3be262fd7cb68339f0377a45e8227fe332",
)

load("@rules_rust//rust:repositories.bzl", "rust_repositories")
rust_repositories(version = "nightly", iso_date = "2021-11-01", edition="2021", dev_components = True)

load("//third_party:crates.bzl", "raze_fetch_remote_crates")
raze_fetch_remote_crates()

load("@rules_rust//proto:repositories.bzl", "rust_proto_repositories")
rust_proto_repositories()

# @com_google_protobuf is loaded from `rust_proto_repositories`.
load("@com_google_protobuf//:protobuf_deps.bzl", "protobuf_deps")
protobuf_deps()
