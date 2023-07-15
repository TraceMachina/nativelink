workspace(name = "turbo-cache")

load("@bazel_tools//tools/build_defs/repo:http.bzl", "http_archive")

http_archive(
    name = "rules_rust",
    sha256 = "4a9cb4fda6ccd5b5ec393b2e944822a62e050c7c06f1ea41607f14c4fdec57a2",
    urls = [
        "https://github.com/bazelbuild/rules_rust/releases/download/0.25.1/rules_rust-v0.25.1.tar.gz",
    ],
)

http_archive(
    name = "rules_python",
    sha256 = "84aec9e21cc56fbc7f1335035a71c850d1b9b5cc6ff497306f84cced9a769841",
    strip_prefix = "rules_python-0.23.1",
    urls = [
        "https://github.com/bazelbuild/rules_python/releases/download/0.23.1/rules_python-0.23.1.tar.gz",
    ],
)

load(
    "@rules_rust//rust:repositories.bzl",
    "rules_rust_dependencies",
    "rust_register_toolchains",
)

rules_rust_dependencies()

rust_register_toolchains(
    edition = "2021",
    versions = [
        "1.70.0",
        "nightly/2023-07-15",
    ],
)

load(
    "@rules_rust//crate_universe:defs.bzl",
    "crates_repository",
    "render_config",
)

crates_repository(
    name = "crate_index",
    cargo_lockfile = "//:Cargo.lock",
    lockfile = "//:Cargo.Bazel.lock",
    manifests = ["//:Cargo.toml"],
    render_config = render_config(
        default_package_name = "",
    ),
    supported_platform_triples = [
        "aarch64-unknown-linux-gnu",
        "arm-unknown-linux-gnueabi",
        "armv7-unknown-linux-gnueabi",
        "x86_64-unknown-linux-gnu",
    ],
)

load("@crate_index//:defs.bzl", "crate_repositories")

crate_repositories()

http_archive(
    name = "com_google_protobuf",
    sha256 = "a700a49470d301f1190a487a923b5095bf60f08f4ae4cac9f5f7c36883d17971",
    strip_prefix = "protobuf-23.4",
    urls = [
        "https://github.com/protocolbuffers/protobuf/releases/download/v23.4/protobuf-23.4.tar.gz",
    ],
)

load("@com_google_protobuf//:protobuf_deps.bzl", "protobuf_deps")

# The version of utf8_range provided by com_google_protobuf raises warnings
# which have been cleaned up in this slightly newer commit.
http_archive(
    name = "utf8_range",
    sha256 = "568988b5f7261ca181468dba38849fabf59dd9200fb2ed4b2823da187ef84d8c",
    strip_prefix = "utf8_range-d863bc33e15cba6d873c878dcca9e6fe52b2f8cb",
    urls = [
        "https://github.com/protocolbuffers/utf8_range/archive/d863bc33e15cba6d873c878dcca9e6fe52b2f8cb.zip",
    ],
)

protobuf_deps()
