load("@rules_rust//rust:defs.bzl", "rust_library", "rust_test")

rust_library(
    name = "platform_property_manager",
    srcs = ["platform_property_manager.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        "//config",
        "//proto",
        "//util:error",
    ],
)

rust_library(
    name = "scheduler",
    srcs = ["scheduler.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        ":worker",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "mock_scheduler",
    testonly = True,
    srcs = ["tests/utils/mock_scheduler.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        ":scheduler",
        "//util:error",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "scheduler_utils",
    testonly = True,
    srcs = ["tests/utils/scheduler_utils.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        "//util:common",
    ],
)

rust_library(
    name = "simple_scheduler",
    srcs = ["simple_scheduler.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        ":scheduler",
        ":worker",
        "//config",
        "//util:common",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:parking_lot",
        "@crate_index//:futures",
        "@crate_index//:hashbrown",
        "@crate_index//:lru",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "cache_lookup_scheduler",
    srcs = ["cache_lookup_scheduler.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        ":scheduler",
        ":worker",
        "//cas/store",
        "//cas/store:ac_utils",
        "//cas/store:grpc_store",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:scopeguard",
        "@crate_index//:futures",
        "@crate_index//:parking_lot",
        "@crate_index//:tonic",
        "@crate_index//:tokio",
        "@crate_index//:tokio-stream",
    ],
)

rust_test(
    name = "cache_lookup_scheduler_test",
    srcs = ["tests/cache_lookup_scheduler_test.rs"],
    deps = [
        ":action_messages",
        ":cache_lookup_scheduler",
        ":mock_scheduler",
        ":platform_property_manager",
        ":scheduler",
        ":scheduler_utils",
        "//cas/store",
        "//cas/store:memory_store",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:prost",
        "@crate_index//:tokio",
        "@crate_index//:tokio-stream",
    ],
)

rust_library(
    name = "property_modifier_scheduler",
    srcs = ["property_modifier_scheduler.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        ":scheduler",
        "//config",
        "//util:error",
        "@crate_index//:parking_lot",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "property_modifier_scheduler_test",
    srcs = ["tests/property_modifier_scheduler_test.rs"],
    deps = [
        ":action_messages",
        ":property_modifier_scheduler",
        ":mock_scheduler",
        ":platform_property_manager",
        ":scheduler",
        ":scheduler_utils",
        "//config",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "grpc_scheduler",
    srcs = ["grpc_scheduler.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    proc_macro_deps = ["@crate_index//:async-trait"],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        ":scheduler",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:parking_lot",
        "@crate_index//:tokio",
        "@crate_index//:tonic",
    ],
)

rust_library(
    name = "default_scheduler_factory",
    srcs = ["default_scheduler_factory.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":cache_lookup_scheduler",
        ":grpc_scheduler",
        ":property_modifier_scheduler",
        ":scheduler",
        ":simple_scheduler",
        "//cas/store",
        "//config",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:futures",
        "@crate_index//:tokio",
    ],
)

rust_library(
    name = "worker",
    srcs = ["worker.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        "//proto",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:tokio",
        "@crate_index//:uuid",
    ],
)

rust_library(
    name = "action_messages",
    srcs = ["action_messages.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":platform_property_manager",
        "//proto",
        "//util:common",
        "//util:error",
        "//util:metrics_utils",
        "@crate_index//:prost",
        "@crate_index//:prost-types",
        "@crate_index//:sha2",
        "@crate_index//:tonic",
    ],
)

rust_test(
    name = "action_messages_test",
    srcs = ["tests/action_messages_test.rs"],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)

rust_test(
    name = "simple_scheduler_test",
    srcs = ["tests/simple_scheduler_test.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        ":scheduler",
        ":scheduler_utils",
        ":simple_scheduler",
        ":worker",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)
