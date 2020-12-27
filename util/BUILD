# Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

load("@io_bazel_rules_rust//rust:rust.bzl", "rust_library")

rust_library(
    name = "macros",
    srcs = ["macros.rs"],
    deps = [
        "//third_party:tokio",
    ],
    visibility = ["//visibility:public"],
)

rust_library(
    name = "common",
    srcs = ["common.rs"],
    deps = [
        "//third_party:tokio",
        "//third_party:tonic",
        "//proto",
    ],
    visibility = ["//visibility:public"],
)
