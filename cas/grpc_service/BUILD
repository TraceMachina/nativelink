load("@io_bazel_rules_rust//rust:rust.bzl", "rust_library", "rust_test")

rust_library(
    name = "cas_server",
    srcs = ["cas_server.rs"],
    deps = [
        "//proto",
        "//util:macros",
        "//third_party:tonic",
        "//third_party:tokio",
        "//third_party:futures_core",
        "//third_party:stdext",
        "//cas/store",
        "//util:common",
    ],
    visibility = ["//cas:__pkg__"]
)

rust_library(
    name = "ac_server",
    srcs = ["ac_server.rs"],
    deps = [
        "//proto",
        "//cas/store",
        "//third_party:stdext",
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
        "//third_party:stdext",
        "//third_party:futures_core",
    ],
    visibility = ["//cas:__pkg__"]
)

rust_test(
    name = "cas_server_test",
    srcs = ["tests/cas_server_test.rs"],
    deps = [
        ":cas_server",
        "//cas/store",
        "//proto",
        "//third_party:tonic",
        "//third_party:tokio",
    ],
)

rust_test(
    name = "ac_server_test",
    srcs = ["tests/ac_server_test.rs"],
    deps = [
        ":ac_server",
        "//cas/store",
        "//util:macros",
        "//proto",
        "//third_party:tonic",
        "//third_party:tokio",
    ],
)
