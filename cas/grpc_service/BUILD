load("@rules_rust//rust:defs.bzl", "rust_library", "rust_test")

rust_library(
    name = "cas_server",
    srcs = ["cas_server.rs"],
    visibility = ["//cas:__pkg__"],
    deps = [
        "//cas/store",
        "//cas/store:grpc_store",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:bytes",
        "@crate_index//:futures",
        "@crate_index//:stdext",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
    ],
)

rust_library(
    name = "ac_server",
    srcs = ["ac_server.rs"],
    visibility = ["//cas:__pkg__"],
    deps = [
        "//cas/store",
        "//cas/store:ac_utils",
        "//cas/store:grpc_store",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:bytes",
        "@crate_index//:prost",
        "@crate_index//:tonic",
    ],
)

rust_library(
    name = "capabilities_server",
    srcs = ["capabilities_server.rs"],
    visibility = ["//cas:__pkg__"],
    deps = [
        "//cas/scheduler",
        "//cas/store",
        "//config",
        "//proto",
        "//util:error",
        "@crate_index//:tonic",
    ],
)

rust_library(
    name = "bytestream_server",
    srcs = ["bytestream_server.rs"],
    visibility = ["//cas:__pkg__"],
    deps = [
        "//cas/store",
        "//cas/store:grpc_store",
        "//config",
        "//proto",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:resource_info",
        "//util:write_request_stream_wrapper",
        "@crate_index//:async-lock",
        "@crate_index//:futures",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
    ],
)

rust_library(
    name = "execution_server",
    srcs = ["execution_server.rs"],
    visibility = ["//cas:__pkg__"],
    deps = [
        "//cas/scheduler",
        "//cas/scheduler:action_messages",
        "//cas/scheduler:platform_property_manager",
        "//cas/store",
        "//cas/store:ac_utils",
        "//cas/store:grpc_store",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:bytes",
        "@crate_index//:futures",
        "@crate_index//:prost",
        "@crate_index//:rand",
        "@crate_index//:stdext",
        "@crate_index//:tokio",
        "@crate_index//:tokio-stream",
        "@crate_index//:tonic",
    ],
)

rust_library(
    name = "worker_api_server",
    srcs = ["worker_api_server.rs"],
    visibility = ["//cas:__pkg__"],
    deps = [
        "//cas/scheduler",
        "//cas/scheduler:action_messages",
        "//cas/scheduler:platform_property_manager",
        "//cas/scheduler:worker",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:futures",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
        "@crate_index//:uuid",
    ],
)

rust_test(
    name = "worker_api_server_test",
    srcs = ["tests/worker_api_server_test.rs"],
    deps = [
        ":worker_api_server",
        "//cas/scheduler",
        "//cas/scheduler:action_messages",
        "//cas/scheduler:platform_property_manager",
        "//cas/scheduler:worker",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
        "@crate_index//:tokio-stream",
        "@crate_index//:tonic",
    ],
)

rust_test(
    name = "cas_server_test",
    srcs = ["tests/cas_server_test.rs"],
    deps = [
        ":cas_server",
        "//cas/store",
        "//cas/store:default_store_factory",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:maplit",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
    ],
)

rust_test(
    name = "ac_server_test",
    srcs = ["tests/ac_server_test.rs"],
    deps = [
        ":ac_server",
        "//cas/store",
        "//cas/store:default_store_factory",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:bytes",
        "@crate_index//:maplit",
        "@crate_index//:pretty_assertions",
        "@crate_index//:prost",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
    ],
)

rust_test(
    name = "bytestream_server_test",
    srcs = ["tests/bytestream_server_test.rs"],
    deps = [
        ":bytestream_server",
        "//cas/store",
        "//cas/store:default_store_factory",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:futures",
        "@crate_index//:maplit",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
        "@crate_index//:tokio-stream",
        "@crate_index//:tonic",
    ],
)
