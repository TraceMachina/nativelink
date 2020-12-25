# Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

load("@io_bazel_rules_rust//rust:rust.bzl", "rust_binary")

rust_binary(
    name = "helloworld",
    srcs = ["helloworld_main.rs"],
    deps = [
        "//proto",
        "//third_party:tonic",
        "//third_party:tokio",
        "//third_party:futures_core",
    ],
)
