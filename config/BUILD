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
        ":schedulers",
        ":stores",
        ":cas_server",
        "@crate_index//:serde",
    ],
)

rust_library(
    name = "cas_server",
    srcs = ["cas_server.rs"],
    deps = [
        ":schedulers",
        ":stores",
        "//util:serde_utils",
        "@crate_index//:serde",
    ],
)

rust_library(
    name = "stores",
    srcs = ["stores.rs"],
    deps = [
        "//util:serde_utils",
        "@crate_index//:serde",
    ],
)

rust_library(
    name = "schedulers",
    srcs = ["schedulers.rs"],
    deps = [
        ":stores",
        "//util:serde_utils",
        "@crate_index//:serde",
    ],
)
