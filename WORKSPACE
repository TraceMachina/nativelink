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
    "crate",
)

crates_repository(
    name = "crate_index",
    cargo_lockfile = "//:Cargo.lock",
    lockfile = "//:Cargo.Bazel.lock",
    packages = {
        "prost": crate.spec(
            version = "0.11.9",
        ),
        "prost-types": crate.spec(
            version = "0.11.9",
        ),
        "hex": crate.spec(
            version = "0.4.3",
        ),
        "async-trait": crate.spec(
            version = "0.1.71",
        ),
        "fixed-buffer": crate.spec(
            version = "0.2.3",
        ),
        "futures": crate.spec(
            version = "0.3.28",
        ),
        "tokio": crate.spec(
            version = "1.29.1",
            features = ["macros", "io-util", "fs", "rt-multi-thread", "parking_lot"],
        ),
        "tokio-stream": crate.spec(
            version = "0.1.14",
            features = ["fs", "sync"],
        ),
        "tokio-util": crate.spec(
            version = "0.7.8",
            features = ["io", "io-util", "codec"],
        ),
        "tonic": crate.spec(
            version = "0.9.2",
            features = ["gzip"],
        ),
        "log": crate.spec(
            version = "0.4.19",
        ),
        "env_logger": crate.spec(
            version = "0.10.0",
        ),
        "serde": crate.spec(
            version = "1.0.167",
        ),
        "json5": crate.spec(
            version = "0.4.1",
        ),
        "sha2": crate.spec(
            version = "0.10.7",
        ),
        "lru": crate.spec(
            version = "0.10.1",
        ),
        "rand": crate.spec(
            version = "0.8.5",
        ),
        "rusoto_s3": crate.spec(
            version = "0.48.0",
        ),
        "rusoto_core": crate.spec(
            version = "0.48.0",
        ),
        "rusoto_signature": crate.spec(
            version = "0.48.0",
        ),
        "http": crate.spec(
            version = "^0.2",
        ),
        "pin-project-lite": crate.spec(
            version = "0.2.10",
        ),
        "async-lock": crate.spec(
            version = "2.7.0",
        ),
        "lz4_flex": crate.spec(
            version = "0.11.1",
        ),
        "bincode": crate.spec(
            version = "1.3.3",
        ),
        "bytes": crate.spec(
            version = "1.4.0",
        ),
        "shellexpand": crate.spec(
            version = "3.1.0",
        ),
        "byteorder": crate.spec(
            version = "1.4.3",
        ),
        "lazy_static": crate.spec(
            version = "1.4.0",
        ),
        "filetime": crate.spec(
            version = "0.2.21",
        ),
        "nix": crate.spec(
            version = "0.26.2",
        ),
        "clap": crate.spec(
            version = "4.3.11",
            features = ["derive"],
        ),
        "uuid": crate.spec(
            version = "1.4.0",
            features = ["v4"],
        ),
        "shlex": crate.spec(
            version = "1.1.0",
        ),
        "relative-path": crate.spec(
            version = "1.8.0",
        ),
        "parking_lot": crate.spec(
            version = "0.12.1",
        ),
        "hashbrown": crate.spec(
            version = "0.14",
        ),
        "hyper": crate.spec(
            version = "0.14.27",
        ),
        "axum": crate.spec(
            version = "0.6.18",
        ),
        "tower": crate.spec(
            version = "0.4.13",
        ),
        "prometheus-client": crate.spec(
            version = "0.21.2",
        ),
        "blake3": crate.spec(
            version = "1.4.1",
        ),
        "drop_guard": crate.spec(
            version = "0.3.0",
        ),
        "stdext": crate.spec(
            version = "0.3.1",
        ),
        "prost-build": crate.spec(
            version = "0.11.9",
        ),
        "tonic-build": crate.spec(
            version = "0.9.2",
        ),
        "pretty_assertions": crate.spec(
            version = "1.4.0",
        ),
        "maplit": crate.spec(
            version = "1.0.2",
        ),
        "mock_instant": crate.spec(
            version = "0.3.1",
        ),
        "rusoto_mock": crate.spec(
            version = "=0.48.0",
        ),
        "ctor": crate.spec(
            version = "0.2.3",
        ),
    },
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
