# Copyright 2021 Nathan (Blaise) Bruer.  All rights reserved.

load("@rules_rust//rust:defs.bzl", "rust_library")

exports_files(["examples/basic_cas.json"])

rust_library(
    name = "config",
    srcs = ["lib.rs"],
    deps = [
        ":backends",
        ":cas_server",
        "//third_party:serde",
    ],
    visibility = ["//visibility:public"],
)

rust_library(
    name = "cas_server",
    srcs = ["cas_server.rs"],
    deps = [
        ":backends",
        "//third_party:serde",
    ],
)

rust_library(
    name = "backends",
    srcs = ["backends.rs"],
    deps = ["//third_party:serde"],
)
