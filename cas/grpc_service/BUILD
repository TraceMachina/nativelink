load("@io_bazel_rules_rust//rust:rust.bzl", "rust_library")

rust_library(
    name = "cas_server",
    srcs = ["cas_server.rs"],
    deps = [
        "//proto",
        "//third_party:tonic",
        "//third_party:futures_core",
    ],
    visibility = ["//cas:__pkg__"]
)
