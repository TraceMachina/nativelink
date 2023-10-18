workspace(name = "turbo-cache")

load("@bazel_tools//tools/build_defs/repo:http.bzl", "http_archive")

http_archive(
    name = "rules_rust",
    sha256 = "814680e1ab535f799fd10e8739ddca901351ceb4d2d86dd8126c22d36e9fcbd9",
    urls = [
        "https://github.com/bazelbuild/rules_rust/releases/download/0.29.0/rules_rust-v0.29.0.tar.gz",
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

load("//tools:cargo_shared.bzl", "PACKAGES", "RUST_EDITION")

rust_register_toolchains(
    edition = RUST_EDITION,
    versions = [
        "1.70.0",
        "nightly/2023-07-15",
    ],
)

load(
    "@rules_rust//crate_universe:defs.bzl",
    "crate",
    "crates_repository",
    "render_config",
)

crates_repository(
    name = "crate_index",
    cargo_lockfile = "//:Cargo.lock",
    lockfile = "//:Cargo.Bazel.lock",
    packages = {
        name: crate.spec(
            features = PACKAGES[name].get("features", []),
            version = PACKAGES[name]["version"],
        )
        for name in PACKAGES
    },
    render_config = render_config(
        default_package_name = "",
    ),
    supported_platform_triples = [
        "aarch64-unknown-linux-gnu",
        "arm-unknown-linux-gnueabi",
        "armv7-unknown-linux-gnueabi",
        "x86_64-unknown-linux-gnu",
        "x86_64-pc-windows-msvc",
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
