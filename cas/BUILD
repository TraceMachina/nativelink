load("@io_bazel_rules_rust//rust:rust.bzl", "rust_binary")

rust_binary(
    name = "cas",
    srcs = ["cas_main.rs"],
    deps = [
        "//cas/grpc_service:cas_server",
        "//cas/grpc_service:ac_server",
        "//cas/grpc_service:capabilities_server",
        "//cas/grpc_service:execution_server",
        "//cas/grpc_service:bytestream_server",
        "//cas/store",
        "//third_party:tokio",
        "//third_party:tonic",
    ],
)
