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

rust_library(
    name = "ac_server",
    srcs = ["ac_server.rs"],
    deps = [
        "//proto",
        "//third_party:tonic",
    ],
    visibility = ["//cas:__pkg__"]
)

rust_library(
    name = "capabilities_server",
    srcs = ["capabilities_server.rs"],
    deps = [
        "//proto",
        "//third_party:tonic",
    ],
    visibility = ["//cas:__pkg__"]
)

rust_library(
    name = "execution_server",
    srcs = ["execution_server.rs"],
    deps = [
        "//proto",
        "//third_party:tonic",
        "//third_party:futures_core",
    ],
    visibility = ["//cas:__pkg__"]
)
