load("@rules_rust//rust:defs.bzl", "rust_library")

exports_files([
    "examples/basic_cas.json",
    "examples/filesystem_cas.json",
    "examples/s3_backend_with_local_fast_cas.json",
])

rust_library(
    name = "config",
    srcs = [
        "lib.rs",
        "cas_server.rs",
        "stores.rs",
        "schedulers.rs",
    ],
    visibility = ["//visibility:public"],
    deps = [
        "//util:serde_utils",
        "@crate_index//:serde",
    ],
)
