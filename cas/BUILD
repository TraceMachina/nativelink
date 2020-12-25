load("@io_bazel_rules_rust//rust:rust.bzl", "rust_binary")

rust_binary(
    name = "cas",
    srcs = ["cas_main.rs"],
    deps = [
        "//cas/grpc_service:cas_server",
        "//third_party:tokio",
        "//third_party:tonic",
    ],
)
