load("@rules_rust//rust:defs.bzl", "rust_binary")

rust_binary(
    name = "cas",
    srcs = ["cas_main.rs"],
    data = ["//config:examples/basic_cas.json"],
    deps = [
        "//cas/grpc_service:ac_server",
        "//cas/grpc_service:bytestream_server",
        "//cas/grpc_service:capabilities_server",
        "//cas/grpc_service:cas_server",
        "//cas/grpc_service:execution_server",
        "//cas/grpc_service:worker_api_server",
        "//cas/scheduler:default_scheduler_factory",
        "//cas/store",
        "//cas/store:default_store_factory",
        "//cas/worker:local_worker",
        "//config",
        "//util:common",
        "//util:error",
        "@crate_index//:clap",
        "@crate_index//:env_logger",
        "@crate_index//:futures",
        "@crate_index//:json5",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
        "@rules_rust//tools/runfiles",
    ],
)
