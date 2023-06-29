load("@rules_rust//rust:defs.bzl", "rust_library")

exports_files([
    "examples/basic_cas.json",
    "examples/filesystem_cas.json",
    "examples/s3_backend_with_local_fast_cas.json",
])

rust_library(
    name = "config",
    srcs = ["lib.rs"],
    visibility = ["//visibility:public"],
    deps = [
        ":backends",
        ":cas_server",
        "@crate_index//:serde",
    ],
)

rust_library(
    name = "cas_server",
    srcs = ["cas_server.rs"],
    deps = [
        ":backends",
        "//util:serde_utils",
        "@crate_index//:serde",
    ],
)

rust_library(
    name = "backends",
    srcs = ["backends.rs"],
    deps = [
        "//util:serde_utils",
        "@crate_index//:serde",
    ],
)
