load("@rules_rust//rust:defs.bzl", "rust_library", "rust_test")

rust_library(
    name = "local_worker",
    srcs = ["local_worker.rs"],
    visibility = ["//cas:__pkg__"],
    deps = [
        ":running_actions_manager",
        ":worker_api_client_wrapper",
        ":worker_utils",
        "//cas/scheduler:action_messages",
        "//cas/store",
        "//cas/store:fast_slow_store",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:async-lock",
        "@crate_index//:futures",
        "@crate_index//:tokio",
        "@crate_index//:tokio-stream",
        "@crate_index//:tonic",
        "@crate_index//:tracing",
    ],
)

rust_library(
    name = "running_actions_manager",
    srcs = ["running_actions_manager.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        "//cas/scheduler:action_messages",
        "//cas/store",
        "//cas/store:ac_utils",
        "//cas/store:fast_slow_store",
        "//cas/store:filesystem_store",
        "//cas/store:grpc_store",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:bytes",
        "@crate_index//:filetime",
        "@crate_index//:formatx",
        "@crate_index//:futures",
        "@crate_index//:hex",
        "@crate_index//:json5",
        "@crate_index//:parking_lot",
        "@crate_index//:prost",
        "@crate_index//:relative-path",
        "@crate_index//:scopeguard",
        "@crate_index//:serde",
        "@crate_index//:tokio",
        "@crate_index//:tokio-stream",
        "@crate_index//:tonic",
        "@crate_index//:tracing",
        "@crate_index//:uuid",
    ],
)

rust_library(
    name = "worker_utils",
    srcs = ["worker_utils.rs"],
    deps = [
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:futures",
        "@crate_index//:shlex",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
        "@crate_index//:tracing",
    ],
)

rust_library(
    name = "worker_api_client_wrapper",
    srcs = ["worker_api_client_wrapper.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        "//proto",
        "@crate_index//:tonic",
    ],
)

rust_library(
    name = "local_worker_test_utils",
    testonly = True,
    srcs = ["tests/utils/local_worker_test_utils.rs"],
    deps = [
        ":local_worker",
        ":mock_running_actions_manager",
        ":mock_worker_api_client",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:hyper",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
    ],
)

rust_library(
    name = "mock_worker_api_client",
    testonly = True,
    srcs = ["tests/utils/mock_worker_api_client.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        ":worker_api_client_wrapper",
        "//proto",
        "@crate_index//:async-lock",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
    ],
)

rust_library(
    name = "mock_running_actions_manager",
    testonly = True,
    srcs = ["tests/utils/mock_running_actions_manager.rs"],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        ":running_actions_manager",
        "//cas/scheduler:action_messages",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:async-lock",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "running_actions_manager_test",
    srcs = ["tests/running_actions_manager_test.rs"],
    deps = [
        ":running_actions_manager",
        "//cas/scheduler:action_messages",
        "//cas/store",
        "//cas/store:ac_utils",
        "//cas/store:fast_slow_store",
        "//cas/store:filesystem_store",
        "//cas/store:memory_store",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:futures",
        "@crate_index//:once_cell",
        "@crate_index//:pretty_assertions",
        "@crate_index//:prost",
        "@crate_index//:prost-types",
        "@crate_index//:rand",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "local_worker_test",
    srcs = ["tests/local_worker_test.rs"],
    proc_macro_deps = ["@crate_index//:ctor"],
    deps = [
        ":local_worker",
        ":local_worker_test_utils",
        ":mock_running_actions_manager",
        "//cas/scheduler:action_messages",
        "//cas/scheduler:platform_property_manager",
        "//cas/store:fast_slow_store",
        "//cas/store:filesystem_store",
        "//cas/store:memory_store",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:prost",
        "@crate_index//:rand",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
    ],
)
