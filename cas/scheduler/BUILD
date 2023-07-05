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
    deps = [
        ":action_messages",
        ":platform_property_manager",
        ":worker",
        "//config",
        "//util:common",
        "//util:error",
        "@crate_index//:parking_lot",
        "@crate_index//:futures",
        "@crate_index//:lru",
        "@crate_index//:rand",
        "@crate_index//:tokio",
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
        ":scheduler",
        "//config",
        "//util:error",
        "@crate_index//:futures",
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
    name = "scheduler_test",
    srcs = ["tests/scheduler_test.rs"],
    visibility = [
        "//cas:__pkg__",
        "//cas:__subpackages__",
    ],
    deps = [
        ":action_messages",
        ":platform_property_manager",
        ":scheduler",
        ":worker",
        "//config",
        "//proto",
        "//util:common",
        "//util:error",
        "@crate_index//:pretty_assertions",
        "@crate_index//:tokio",
    ],
)
