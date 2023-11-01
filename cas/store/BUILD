load("@rules_rust//rust:defs.bzl", "rust_library", "rust_test")

rust_library(
    name = "store",
    srcs = ["lib.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":traits",
        "//config",
        "//util:error",
    ],
)

rust_library(
    name = "default_store_factory",
    srcs = ["default_store_factory.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":compression_store",
        ":dedup_store",
        ":fast_slow_store",
        ":filesystem_store",
        ":grpc_store",
        ":memory_store",
        ":ref_store",
        ":s3_store",
        ":shard_store",
        ":size_partitioning_store",
        ":redis_store",
        ":store",
        ":traits",
        ":verify_store",
        "//config",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:futures",
    ],
)

rust_library(
    name = "traits",
    srcs = ["store_trait.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = ["//cas:__pkg__"],
    deps = [
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:bytes",
        "@crate_index//:futures",
        "@crate_index//:serde",
    ],
)

rust_library(
    name = "ac_utils",
    srcs = ["ac_utils.rs"],
    visibility = ["//visibility:public"],
    deps = [
        ":store",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "@crate_index//:bytes",
        "@crate_index//:futures",
        "@crate_index//:prost",
        "@crate_index//:sha2",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "size_partitioning_store",
    srcs = ["size_partitioning_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = ["//cas:__pkg__"],
    deps = [
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "ref_store",
    srcs = ["ref_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = ["//cas:__pkg__"],
    deps = [
        ":store",
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
    ],
)

rust_library(
    name = "memory_store",
    srcs = ["memory_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:evicting_map",
        "//util:metrics_utils",
        "@crate_index//:bytes",
    ],
)

rust_library(
    name = "verify_store",
    srcs = ["verify_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = ["//cas:__pkg__"],
    deps = [
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:hex",
        "@crate_index//:sha2",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "fast_slow_store",
    srcs = ["fast_slow_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:bytes",
        "@crate_index//:futures",
    ],
)

rust_library(
    name = "shard_store",
    srcs = ["shard_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:futures",
    ],
)

rust_library(
    name = "dedup_store",
    srcs = ["dedup_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = ["//cas:__pkg__"],
    deps = [
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:fastcdc",
        "@crate_index//:bincode",
        "@crate_index//:futures",
        "@crate_index//:serde",
        "@crate_index//:blake3",
        "@crate_index//:tokio",
        "@crate_index//:tokio-util",
    ],
)

rust_library(
    name = "compression_store",
    srcs = ["compression_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = ["//cas:__pkg__"],
    deps = [
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "@crate_index//:bincode",
        "@crate_index//:byteorder",
        "@crate_index//:bytes",
        "@crate_index//:futures",
        "@crate_index//:lz4_flex",
        "@crate_index//:serde",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "s3_store",
    srcs = ["s3_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = ["//cas:__pkg__"],
    deps = [
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:retry",
        "@crate_index//:bytes",
        "@crate_index//:futures",
        "@crate_index//:http",
        "@crate_index//:lazy_static",
        "@crate_index//:rand",
        "@crate_index//:rusoto_core",
        "@crate_index//:rusoto_s3",
        "@crate_index//:rusoto_signature",
        "@crate_index//:tokio",
        "@crate_index//:tokio-util",
    ],
)

rust_library(
    name = "filesystem_store",
    srcs = ["filesystem_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:evicting_map",
        "//util:metrics_utils",
        "@crate_index//:async-lock",
        "@crate_index//:bytes",
        "@crate_index//:filetime",
        "@crate_index//:futures",
        "@crate_index//:nix",
        "@crate_index//:prometheus-client",
        "@crate_index//:rand",
        "@crate_index//:tokio",
        "@crate_index//:tokio-stream",
    ],
)

rust_library(
    name = "grpc_store",
    srcs = ["grpc_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":ac_utils",
        ":traits",
        "//config",
        "//proto",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:retry",
        "//util:write_request_stream_wrapper",
        "@crate_index//:bytes",
        "@crate_index//:futures",
        "@crate_index//:parking_lot",
        "@crate_index//:prost",
        "@crate_index//:rand",
        "@crate_index//:shellexpand",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
        "@crate_index//:uuid",
    ],
)

rust_library(
    name = "redis_store",
    srcs = ["redis_store.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    visibility = ["//cas:__pkg__"],
    deps = [
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "@crate_index//:tokio",
        "@crate_index//:sha2",
        "@crate_index//:bytes",
        "@crate_index//:redis",
        "@crate_index//:pretty_assertions",
    ]
)

rust_test(
    name = "size_partitioning_store_test",
    srcs = ["tests/size_partitioning_store_test.rs"],
    deps = [
        ":memory_store",
        ":size_partitioning_store",
        ":traits",
        "//config",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "redis_store_test",
    srcs = ["tests/redis_store_test.rs"],
    deps = [
        ":traits",
        ":redis_store",
        ":s3_store",
        "//config",
        "//util:buf_channel",
        "//util:async_fixed_buffer",
        "//util:common",
        "//util:error",
        "@crate_index//:redis",
        "@crate_index//:tokio",
        "@crate_index//:redis-test",
        "@crate_index//:pretty_assertions",
    ],
)

rust_test(
    name = "ref_store_test",
    srcs = ["tests/ref_store_test.rs"],
    deps = [
        ":memory_store",
        ":ref_store",
        ":store",
        "//config",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "fast_slow_store_test",
    srcs = ["tests/fast_slow_store_test.rs"],
    deps = [
        ":fast_slow_store",
        ":memory_store",
        ":traits",
        "//config",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:rand",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "shard_store_test",
    srcs = ["tests/shard_store_test.rs"],
    deps = [
        ":shard_store",
        ":memory_store",
        ":traits",
        "//config",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:rand",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "memory_store_test",
    srcs = ["tests/memory_store_test.rs"],
    env = {
        "RUST_TEST_THREADS": "1", # This test can't run in parallel.
    },
    deps = [
        ":memory_store",
        ":traits",
        "//config",
        "//util:common",
        "//util:error",
        "@crate_index//:bytes",
        "@crate_index//:memory-stats",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "verify_store_test",
    srcs = ["tests/verify_store_test.rs"],
    deps = [
        ":memory_store",
        ":traits",
        ":verify_store",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "@crate_index//:futures",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "s3_store_test",
    srcs = ["tests/s3_store_test.rs"],
    deps = [
        ":s3_store",
        ":traits",
        "//config",
        "//util:async_fixed_buffer",
        "//util:common",
        "//util:error",
        "@crate_index//:futures",
        "@crate_index//:http",
        "@crate_index//:pretty_assertions",
        "@crate_index//:rusoto_core",
        "@crate_index//:rusoto_mock",
        "@crate_index//:rusoto_s3",
        "@crate_index//:tokio",
        "@crate_index//:tokio-util",
    ],
)

rust_test(
    name = "compression_store_test",
    srcs = ["tests/compression_store_test.rs"],
    deps = [
        ":compression_store",
        ":memory_store",
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "@crate_index//:bincode",
        "@crate_index//:futures",
        "@crate_index//:pretty_assertions",
        "@crate_index//:rand",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "dedup_store_test",
    srcs = ["tests/dedup_store_test.rs"],
    deps = [
        ":dedup_store",
        ":memory_store",
        ":traits",
        "//config",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:rand",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "filesystem_store_test",
    srcs = ["tests/filesystem_store_test.rs"],
    deps = [
        ":filesystem_store",
        ":traits",
        "//config",
        "//util:buf_channel",
        "//util:common",
        "//util:error",
        "//util:evicting_map",
        "@crate_index//:async-lock",
        "@crate_index//:filetime",
        "@crate_index//:futures",
        "@crate_index//:lazy_static",
        "@crate_index//:pretty_assertions",
        "@crate_index//:rand",
        "@crate_index//:tokio",
        "@crate_index//:tokio-stream",
    ],
    proc_macro_deps = ["@crate_index//:async-trait"],
)

rust_test(
    name = "ac_utils_test",
    srcs = ["tests/ac_utils_test.rs"],
    deps = [
        ":ac_utils",
        ":memory_store",
        ":store",
        "//config",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:rand",
        "@crate_index//:tokio",
    ],
)