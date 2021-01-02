# Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

load("@io_bazel_rules_rust//rust:rust.bzl", "rust_library", "rust_test")

rust_library(
    name = "store",
    srcs = ["lib.rs"],
    deps = [
        ":memory_store",
        ":traits",
    ],
    visibility = ["//cas:__pkg__", "//cas:__subpackages__"]
)

rust_library(
    name = "traits",
    srcs = ["store_trait.rs"],
    deps = [
        "//third_party:tokio",
        "//util:error",
        "//util:common",
    ],
    proc_macro_deps = ["//third_party:async_trait"],
    visibility = ["//cas:__pkg__"]
)

rust_library(
    name = "memory_store",
    srcs = ["memory_store.rs"],
    deps = [
        ":traits",
        "//util:error",
        "//third_party:async_mutex",
        "//third_party:tokio",
        "//util:common",
    ],
    proc_macro_deps = ["//third_party:async_trait"],
    visibility = ["//cas:__pkg__"]
)

rust_test(
    name = "store_tests",
    srcs = ["tests/memory_store_test.rs"],
    deps = [
        ":memory_store",
        ":traits",
        "//util:error",
        "//util:common",
        "//third_party:tokio",
        "//third_party:pretty_assertions",
    ],
)
