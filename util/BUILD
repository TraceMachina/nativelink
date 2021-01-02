# Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

load("@io_bazel_rules_rust//rust:rust.bzl", "rust_library")

rust_library(
    name = "error",
    srcs = ["error.rs"],
    deps = [
        "//third_party:hex",
        "//third_party:prost",
        "//third_party:tokio",
        "//third_party:tonic",
        "//proto",
    ],
    visibility = ["//visibility:public"],
)

rust_library(
    name = "common",
    srcs = ["common.rs"],
    deps = [
        ":error",
        "//proto",
        "//third_party:hex",
        "//third_party:lazy_init",
    ],
    visibility = ["//visibility:public"],
)

rust_library(
    name = "async_fixed_buffer",
    srcs = ["async_fixed_buffer.rs"],
    deps = [
        "//third_party:tokio",
        "//third_party:fixed_buffer",
    ],
    visibility = ["//visibility:public"],
)
