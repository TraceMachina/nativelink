load("@io_bazel_rules_rust//rust:rust.bzl", "rust_library", "rust_test", "rust_binary")

rust_library(
    name = "cas_server",
    srcs = ["cas_server.rs"],
    deps = [
        "//proto",
        "//third_party:tonic",
        "//third_party:tokio",
        "//third_party:futures",
        "//third_party:stdext",
        "//cas/store",
        "//util:error",
    ],
    visibility = ["//cas:__pkg__"]
)

rust_library(
    name = "ac_server",
    srcs = ["ac_server.rs"],
    deps = [
        "//proto",
        "//cas/store",
        "//util:error",
        "//third_party:stdext",
        "//third_party:prost",
        "//third_party:tonic",
        "//third_party:tokio",
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
    name = "bytestream_server",
    srcs = ["bytestream_server.rs"],
    deps = [
        "//proto",
        "//third_party:tonic",
        "//util:async_fixed_buffer",
        "//cas/store",
        "//third_party:tokio",
        "//third_party:futures",
        "//util:error",
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
        "//third_party:futures",
    ],
    visibility = ["//cas:__pkg__"]
)

rust_test(
    name = "cas_server_test",
    srcs = ["tests/cas_server_test.rs"],
    deps = [
        ":cas_server",
        "//cas/store",
        "//util:error",
        "//proto",
        "//third_party:tonic",
        "//third_party:tokio",
        "//third_party:pretty_assertions",
    ],
)

rust_test(
    name = "ac_server_test",
    srcs = ["tests/ac_server_test.rs"],
    deps = [
        ":ac_server",
        "//cas/store",
        "//proto",
        "//third_party:tonic",
        "//third_party:tokio",
        "//third_party:prost",
        "//third_party:pretty_assertions",
    ],
)

rust_test(
    name = "bytestream_server_test",
    srcs = ["tests/bytestream_server_test.rs"],
    deps = [
        ":bytestream_server",
        "//cas/store",
        "//proto",
        "//util:error",
        "//third_party:tonic",
        "//third_party:bytes",
        "//third_party:tokio",
        "//third_party:prost",
        "//third_party:pretty_assertions",
    ],
)
