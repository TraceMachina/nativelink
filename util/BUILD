# Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

load("@io_bazel_rules_rust//rust:rust.bzl", "rust_library", "rust_test")

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
        "//third_party:log",
        "//third_party:lazy_init",
    ],
    visibility = ["//visibility:public"],
)

rust_library(
    name = "evicting_map",
    srcs = ["evicting_map.rs"],
    deps = [
        "//config",
        "//third_party:lru",
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

rust_test(
    name = "async_fixed_buffer_test",
    srcs = ["tests/async_fixed_buffer_test.rs"],
    deps = [
        "//third_party:futures",
        "//third_party:pretty_assertions",
        "//third_party:tokio",
        ":async_fixed_buffer",
        ":error",
    ],
)

rust_test(
    name = "evicting_map_test",
    srcs = ["tests/evicting_map_test.rs"],
    deps = [
        "//config",
        "//third_party:hex",
        "//third_party:mock_instant",
        "//third_party:tokio",
        ":error",
        ":evicting_map",
    ],
)
