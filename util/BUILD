load("@rules_rust//rust:defs.bzl", "rust_library", "rust_test")

rust_library(
    name = "error",
    srcs = ["error.rs"],
    visibility = ["//visibility:public"],
    deps = [
        "//proto",
        "@crate_index//:hex",
        "@crate_index//:prost",
        "@crate_index//:prost-types",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
    ],
)

rust_library(
    name = "common",
    srcs = ["common.rs"],
    visibility = ["//visibility:public"],
    deps = [
        ":error",
        ":fs",
        "//proto",
        "@crate_index//:bytes",
        "@crate_index//:hex",
        "@crate_index//:log",
        "@crate_index//:prost",
        "@crate_index//:serde",
        "@crate_index//:tokio",
    ],
)

# This crate should be accessed through `common::fs`.
rust_library(
    name = "fs",
    srcs = ["fs.rs"],
    deps = [
        ":error",
        "@crate_index//:log",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "retry",
    srcs = ["retry.rs"],
    visibility = ["//visibility:public"],
    deps = [
        ":error",
        "@crate_index//:futures",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "resource_info",
    srcs = ["resource_info.rs"],
    visibility = ["//visibility:public"],
    deps = [":error"],
)

rust_library(
    name = "write_request_stream_wrapper",
    srcs = ["write_request_stream_wrapper.rs"],
    visibility = ["//visibility:public"],
    deps = [
        ":error",
        ":resource_info",
        "//proto",
        "@crate_index//:futures",
    ],
)

rust_library(
    name = "evicting_map",
    srcs = ["evicting_map.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = ["//visibility:public"],
    deps = [
        "//config",
        "//util:common",
        "@crate_index//:async-lock",
        "@crate_index//:futures",
        "@crate_index//:lru",
        "@crate_index//:serde",
    ],
)

rust_library(
    name = "async_fixed_buffer",
    srcs = ["async_fixed_buffer.rs"],
    visibility = ["//visibility:public"],
    deps = [
        "@crate_index//:fixed-buffer",
        "@crate_index//:futures",
        "@crate_index//:parking_lot",
        "@crate_index//:pin-project-lite",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "write_counter",
    srcs = ["write_counter.rs"],
    visibility = ["//visibility:public"],
    deps = [
        "@crate_index//:pin-project-lite",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "buf_channel",
    srcs = ["buf_channel.rs"],
    visibility = ["//visibility:public"],
    deps = [
        ":error",
        "@crate_index//:bytes",
        "@crate_index//:futures",
        "@crate_index//:tokio",
        "@crate_index//:tokio-util",
    ],
)

rust_library(
    name = "fastcdc",
    srcs = ["fastcdc.rs"],
    visibility = ["//visibility:public"],
    deps = [
        "@crate_index//:bytes",
        "@crate_index//:tokio-util",
    ],
)

rust_library(
    name = "serde_utils",
    srcs = ["serde_utils.rs"],
    visibility = ["//visibility:public"],
    deps = [
        "@crate_index//:serde",
        "@crate_index//:shellexpand",
    ],
)

rust_test(
    name = "resource_info_test",
    srcs = ["tests/resource_info_test.rs"],
    deps = [
        ":resource_info",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "fastcdc_test",
    srcs = ["tests/fastcdc_test.rs"],
    data = ["tests/data/SekienAkashita.jpg"],
    deps = [
        ":fastcdc",
        "//util:common",
        "@crate_index//:bytes",
        "@crate_index//:futures",
        "@crate_index//:hex",
        "@crate_index//:pretty_assertions",
        "@crate_index//:rand",
        "@crate_index//:sha2",
        "@crate_index//:tokio",
        "@crate_index//:tokio-util",
    ],
)

rust_test(
    name = "async_fixed_buffer_test",
    srcs = ["tests/async_fixed_buffer_test.rs"],
    deps = [
        ":async_fixed_buffer",
        ":error",
        "@crate_index//:futures",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "evicting_map_test",
    srcs = ["tests/evicting_map_test.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        ":error",
        ":evicting_map",
        "//config",
        "//util:common",
        "@crate_index//:bytes",
        "@crate_index//:hex",
        "@crate_index//:mock_instant",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "retry_test",
    srcs = ["tests/retry_test.rs"],
    deps = [
        ":error",
        ":retry",
        "@crate_index//:futures",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "buf_channel_test",
    srcs = ["tests/buf_channel_test.rs"],
    deps = [
        ":buf_channel",
        ":error",
        "@crate_index//:bytes",
        "@crate_index//:futures",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)
