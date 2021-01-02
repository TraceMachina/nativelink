"""
@generated
cargo-raze generated Bazel file.

DO NOT EDIT! Replaced on runs of cargo-raze
"""

load("@bazel_tools//tools/build_defs/repo:git.bzl", "new_git_repository")  # buildifier: disable=load
load("@bazel_tools//tools/build_defs/repo:http.bzl", "http_archive")  # buildifier: disable=load
load("@bazel_tools//tools/build_defs/repo:utils.bzl", "maybe")  # buildifier: disable=load

def raze_fetch_remote_crates():
    """This function defines a collection of repos and should be called in a WORKSPACE file"""
    maybe(
        http_archive,
        name = "raze__aho_corasick__0_7_15",
        url = "https://crates.io/api/v1/crates/aho-corasick/0.7.15/download",
        type = "tar.gz",
        sha256 = "7404febffaa47dac81aa44dba71523c9d069b1bdc50a77db41195149e17f68e5",
        strip_prefix = "aho-corasick-0.7.15",
        build_file = Label("//third_party/remote:BUILD.aho-corasick-0.7.15.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__annotate_snippets__0_6_1",
        url = "https://crates.io/api/v1/crates/annotate-snippets/0.6.1/download",
        type = "tar.gz",
        sha256 = "c7021ce4924a3f25f802b2cccd1af585e39ea1a363a1aa2e72afe54b67a3a7a7",
        strip_prefix = "annotate-snippets-0.6.1",
        build_file = Label("//third_party/remote:BUILD.annotate-snippets-0.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__annotate_snippets__0_8_0",
        url = "https://crates.io/api/v1/crates/annotate-snippets/0.8.0/download",
        type = "tar.gz",
        sha256 = "d78ea013094e5ea606b1c05fe35f1dd7ea1eb1ea259908d040b25bd5ec677ee5",
        strip_prefix = "annotate-snippets-0.8.0",
        build_file = Label("//third_party/remote:BUILD.annotate-snippets-0.8.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ansi_term__0_11_0",
        url = "https://crates.io/api/v1/crates/ansi_term/0.11.0/download",
        type = "tar.gz",
        sha256 = "ee49baf6cb617b853aa8d93bf420db2383fab46d314482ca2803b40d5fde979b",
        strip_prefix = "ansi_term-0.11.0",
        build_file = Label("//third_party/remote:BUILD.ansi_term-0.11.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__anyhow__1_0_37",
        url = "https://crates.io/api/v1/crates/anyhow/1.0.37/download",
        type = "tar.gz",
        sha256 = "ee67c11feeac938fae061b232e38e0b6d94f97a9df10e6271319325ac4c56a86",
        strip_prefix = "anyhow-1.0.37",
        build_file = Label("//third_party/remote:BUILD.anyhow-1.0.37.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__arrayref__0_3_6",
        url = "https://crates.io/api/v1/crates/arrayref/0.3.6/download",
        type = "tar.gz",
        sha256 = "a4c527152e37cf757a3f78aae5a06fbeefdb07ccc535c980a3208ee3060dd544",
        strip_prefix = "arrayref-0.3.6",
        build_file = Label("//third_party/remote:BUILD.arrayref-0.3.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__arrayvec__0_5_2",
        url = "https://crates.io/api/v1/crates/arrayvec/0.5.2/download",
        type = "tar.gz",
        sha256 = "23b62fc65de8e4e7f52534fb52b0f3ed04746ae267519eef2a83941e8085068b",
        strip_prefix = "arrayvec-0.5.2",
        build_file = Label("//third_party/remote:BUILD.arrayvec-0.5.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__async_mutex__1_4_0",
        url = "https://crates.io/api/v1/crates/async-mutex/1.4.0/download",
        type = "tar.gz",
        sha256 = "479db852db25d9dbf6204e6cb6253698f175c15726470f78af0d918e99d6156e",
        strip_prefix = "async-mutex-1.4.0",
        build_file = Label("//third_party/remote:BUILD.async-mutex-1.4.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__async_stream__0_2_1",
        url = "https://crates.io/api/v1/crates/async-stream/0.2.1/download",
        type = "tar.gz",
        sha256 = "22068c0c19514942eefcfd4daf8976ef1aad84e61539f95cd200c35202f80af5",
        strip_prefix = "async-stream-0.2.1",
        build_file = Label("//third_party/remote:BUILD.async-stream-0.2.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__async_stream__0_3_0",
        url = "https://crates.io/api/v1/crates/async-stream/0.3.0/download",
        type = "tar.gz",
        sha256 = "3670df70cbc01729f901f94c887814b3c68db038aad1329a418bae178bc5295c",
        strip_prefix = "async-stream-0.3.0",
        build_file = Label("//third_party/remote:BUILD.async-stream-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__async_stream_impl__0_2_1",
        url = "https://crates.io/api/v1/crates/async-stream-impl/0.2.1/download",
        type = "tar.gz",
        sha256 = "25f9db3b38af870bf7e5cc649167533b493928e50744e2c30ae350230b414670",
        strip_prefix = "async-stream-impl-0.2.1",
        build_file = Label("//third_party/remote:BUILD.async-stream-impl-0.2.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__async_stream_impl__0_3_0",
        url = "https://crates.io/api/v1/crates/async-stream-impl/0.3.0/download",
        type = "tar.gz",
        sha256 = "a3548b8efc9f8e8a5a0a2808c5bd8451a9031b9e5b879a79590304ae928b0a70",
        strip_prefix = "async-stream-impl-0.3.0",
        build_file = Label("//third_party/remote:BUILD.async-stream-impl-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__async_trait__0_1_42",
        url = "https://crates.io/api/v1/crates/async-trait/0.1.42/download",
        type = "tar.gz",
        sha256 = "8d3a45e77e34375a7923b1e8febb049bb011f064714a8e17a1a616fef01da13d",
        strip_prefix = "async-trait-0.1.42",
        build_file = Label("//third_party/remote:BUILD.async-trait-0.1.42.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__atty__0_2_14",
        url = "https://crates.io/api/v1/crates/atty/0.2.14/download",
        type = "tar.gz",
        sha256 = "d9b39be18770d11421cdb1b9947a45dd3f37e93092cbf377614828a319d5fee8",
        strip_prefix = "atty-0.2.14",
        build_file = Label("//third_party/remote:BUILD.atty-0.2.14.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__autocfg__1_0_1",
        url = "https://crates.io/api/v1/crates/autocfg/1.0.1/download",
        type = "tar.gz",
        sha256 = "cdb031dd78e28731d87d56cc8ffef4a8f36ca26c38fe2de700543e627f8a464a",
        strip_prefix = "autocfg-1.0.1",
        build_file = Label("//third_party/remote:BUILD.autocfg-1.0.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__base64__0_12_3",
        url = "https://crates.io/api/v1/crates/base64/0.12.3/download",
        type = "tar.gz",
        sha256 = "3441f0f7b02788e948e47f457ca01f1d7e6d92c693bc132c22b087d3141c03ff",
        strip_prefix = "base64-0.12.3",
        build_file = Label("//third_party/remote:BUILD.base64-0.12.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__base64__0_13_0",
        url = "https://crates.io/api/v1/crates/base64/0.13.0/download",
        type = "tar.gz",
        sha256 = "904dfeac50f3cdaba28fc6f57fdcddb75f49ed61346676a78c4ffe55877802fd",
        strip_prefix = "base64-0.13.0",
        build_file = Label("//third_party/remote:BUILD.base64-0.13.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__bitflags__1_2_1",
        url = "https://crates.io/api/v1/crates/bitflags/1.2.1/download",
        type = "tar.gz",
        sha256 = "cf1de2fe8c75bc145a2f577add951f8134889b4795d47466a54a5c846d691693",
        strip_prefix = "bitflags-1.2.1",
        build_file = Label("//third_party/remote:BUILD.bitflags-1.2.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__blake2b_simd__0_5_11",
        url = "https://crates.io/api/v1/crates/blake2b_simd/0.5.11/download",
        type = "tar.gz",
        sha256 = "afa748e348ad3be8263be728124b24a24f268266f6f5d58af9d75f6a40b5c587",
        strip_prefix = "blake2b_simd-0.5.11",
        build_file = Label("//third_party/remote:BUILD.blake2b_simd-0.5.11.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__block_buffer__0_7_3",
        url = "https://crates.io/api/v1/crates/block-buffer/0.7.3/download",
        type = "tar.gz",
        sha256 = "c0940dc441f31689269e10ac70eb1002a3a1d3ad1390e030043662eb7fe4688b",
        strip_prefix = "block-buffer-0.7.3",
        build_file = Label("//third_party/remote:BUILD.block-buffer-0.7.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__block_padding__0_1_5",
        url = "https://crates.io/api/v1/crates/block-padding/0.1.5/download",
        type = "tar.gz",
        sha256 = "fa79dedbb091f449f1f39e53edf88d5dbe95f895dae6135a8d7b881fb5af73f5",
        strip_prefix = "block-padding-0.1.5",
        build_file = Label("//third_party/remote:BUILD.block-padding-0.1.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__bstr__0_2_14",
        url = "https://crates.io/api/v1/crates/bstr/0.2.14/download",
        type = "tar.gz",
        sha256 = "473fc6b38233f9af7baa94fb5852dca389e3d95b8e21c8e3719301462c5d9faf",
        strip_prefix = "bstr-0.2.14",
        build_file = Label("//third_party/remote:BUILD.bstr-0.2.14.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__byte_tools__0_3_1",
        url = "https://crates.io/api/v1/crates/byte-tools/0.3.1/download",
        type = "tar.gz",
        sha256 = "e3b5ca7a04898ad4bcd41c90c5285445ff5b791899bb1b0abdd2a2aa791211d7",
        strip_prefix = "byte-tools-0.3.1",
        build_file = Label("//third_party/remote:BUILD.byte-tools-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__bytecount__0_6_2",
        url = "https://crates.io/api/v1/crates/bytecount/0.6.2/download",
        type = "tar.gz",
        sha256 = "72feb31ffc86498dacdbd0fcebb56138e7177a8cc5cea4516031d15ae85a742e",
        strip_prefix = "bytecount-0.6.2",
        build_file = Label("//third_party/remote:BUILD.bytecount-0.6.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__byteorder__1_3_4",
        url = "https://crates.io/api/v1/crates/byteorder/1.3.4/download",
        type = "tar.gz",
        sha256 = "08c48aae112d48ed9f069b33538ea9e3e90aa263cfa3d1c24309612b1f7472de",
        strip_prefix = "byteorder-1.3.4",
        build_file = Label("//third_party/remote:BUILD.byteorder-1.3.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__bytes__0_5_6",
        url = "https://crates.io/api/v1/crates/bytes/0.5.6/download",
        type = "tar.gz",
        sha256 = "0e4cec68f03f32e44924783795810fa50a7035d8c8ebe78580ad7e6c703fba38",
        strip_prefix = "bytes-0.5.6",
        build_file = Label("//third_party/remote:BUILD.bytes-0.5.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__bytes__1_0_0",
        url = "https://crates.io/api/v1/crates/bytes/1.0.0/download",
        type = "tar.gz",
        sha256 = "ad1f8e949d755f9d79112b5bb46938e0ef9d3804a0b16dfab13aafcaa5f0fa72",
        strip_prefix = "bytes-1.0.0",
        build_file = Label("//third_party/remote:BUILD.bytes-1.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__cargo_metadata__0_8_2",
        url = "https://crates.io/api/v1/crates/cargo_metadata/0.8.2/download",
        type = "tar.gz",
        sha256 = "700b3731fd7d357223d0000f4dbf1808401b694609035c3c411fbc0cd375c426",
        strip_prefix = "cargo_metadata-0.8.2",
        build_file = Label("//third_party/remote:BUILD.cargo_metadata-0.8.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__cc__1_0_66",
        url = "https://crates.io/api/v1/crates/cc/1.0.66/download",
        type = "tar.gz",
        sha256 = "4c0496836a84f8d0495758516b8621a622beb77c0fed418570e50764093ced48",
        strip_prefix = "cc-1.0.66",
        build_file = Label("//third_party/remote:BUILD.cc-1.0.66.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__cfg_if__0_1_10",
        url = "https://crates.io/api/v1/crates/cfg-if/0.1.10/download",
        type = "tar.gz",
        sha256 = "4785bdd1c96b2a846b2bd7cc02e86b6b3dbf14e7e53446c4f54c92a361040822",
        strip_prefix = "cfg-if-0.1.10",
        build_file = Label("//third_party/remote:BUILD.cfg-if-0.1.10.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__cfg_if__1_0_0",
        url = "https://crates.io/api/v1/crates/cfg-if/1.0.0/download",
        type = "tar.gz",
        sha256 = "baf1de4339761588bc0619e3cbc0120ee582ebb74b53b4efbf79117bd2da40fd",
        strip_prefix = "cfg-if-1.0.0",
        build_file = Label("//third_party/remote:BUILD.cfg-if-1.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__clap__2_33_3",
        url = "https://crates.io/api/v1/crates/clap/2.33.3/download",
        type = "tar.gz",
        sha256 = "37e58ac78573c40708d45522f0d80fa2f01cc4f9b4e2bf749807255454312002",
        strip_prefix = "clap-2.33.3",
        build_file = Label("//third_party/remote:BUILD.clap-2.33.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__cloudabi__0_0_3",
        url = "https://crates.io/api/v1/crates/cloudabi/0.0.3/download",
        type = "tar.gz",
        sha256 = "ddfc5b9aa5d4507acaf872de71051dfd0e309860e88966e1051e462a077aac4f",
        strip_prefix = "cloudabi-0.0.3",
        build_file = Label("//third_party/remote:BUILD.cloudabi-0.0.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__constant_time_eq__0_1_5",
        url = "https://crates.io/api/v1/crates/constant_time_eq/0.1.5/download",
        type = "tar.gz",
        sha256 = "245097e9a4535ee1e3e3931fcfcd55a796a44c643e8596ff6566d68f09b87bbc",
        strip_prefix = "constant_time_eq-0.1.5",
        build_file = Label("//third_party/remote:BUILD.constant_time_eq-0.1.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__crossbeam_deque__0_7_3",
        url = "https://crates.io/api/v1/crates/crossbeam-deque/0.7.3/download",
        type = "tar.gz",
        sha256 = "9f02af974daeee82218205558e51ec8768b48cf524bd01d550abe5573a608285",
        strip_prefix = "crossbeam-deque-0.7.3",
        build_file = Label("//third_party/remote:BUILD.crossbeam-deque-0.7.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__crossbeam_epoch__0_8_2",
        url = "https://crates.io/api/v1/crates/crossbeam-epoch/0.8.2/download",
        type = "tar.gz",
        sha256 = "058ed274caafc1f60c4997b5fc07bf7dc7cca454af7c6e81edffe5f33f70dace",
        strip_prefix = "crossbeam-epoch-0.8.2",
        build_file = Label("//third_party/remote:BUILD.crossbeam-epoch-0.8.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__crossbeam_queue__0_1_2",
        url = "https://crates.io/api/v1/crates/crossbeam-queue/0.1.2/download",
        type = "tar.gz",
        sha256 = "7c979cd6cfe72335896575c6b5688da489e420d36a27a0b9eb0c73db574b4a4b",
        strip_prefix = "crossbeam-queue-0.1.2",
        build_file = Label("//third_party/remote:BUILD.crossbeam-queue-0.1.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__crossbeam_utils__0_6_6",
        url = "https://crates.io/api/v1/crates/crossbeam-utils/0.6.6/download",
        type = "tar.gz",
        sha256 = "04973fa96e96579258a5091af6003abde64af786b860f18622b82e026cca60e6",
        strip_prefix = "crossbeam-utils-0.6.6",
        build_file = Label("//third_party/remote:BUILD.crossbeam-utils-0.6.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__crossbeam_utils__0_7_2",
        url = "https://crates.io/api/v1/crates/crossbeam-utils/0.7.2/download",
        type = "tar.gz",
        sha256 = "c3c7c73a2d1e9fc0886a08b93e98eb643461230d5f1925e4036204d5f2e261a8",
        strip_prefix = "crossbeam-utils-0.7.2",
        build_file = Label("//third_party/remote:BUILD.crossbeam-utils-0.7.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__crossbeam_utils__0_8_1",
        url = "https://crates.io/api/v1/crates/crossbeam-utils/0.8.1/download",
        type = "tar.gz",
        sha256 = "02d96d1e189ef58269ebe5b97953da3274d83a93af647c2ddd6f9dab28cedb8d",
        strip_prefix = "crossbeam-utils-0.8.1",
        build_file = Label("//third_party/remote:BUILD.crossbeam-utils-0.8.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ctor__0_1_17",
        url = "https://crates.io/api/v1/crates/ctor/0.1.17/download",
        type = "tar.gz",
        sha256 = "373c88d9506e2e9230f6107701b7d8425f4cb3f6df108ec3042a26e936666da5",
        strip_prefix = "ctor-0.1.17",
        build_file = Label("//third_party/remote:BUILD.ctor-0.1.17.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__derive_new__0_5_8",
        url = "https://crates.io/api/v1/crates/derive-new/0.5.8/download",
        type = "tar.gz",
        sha256 = "71f31892cd5c62e414316f2963c5689242c43d8e7bbcaaeca97e5e28c95d91d9",
        strip_prefix = "derive-new-0.5.8",
        build_file = Label("//third_party/remote:BUILD.derive-new-0.5.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__diff__0_1_12",
        url = "https://crates.io/api/v1/crates/diff/0.1.12/download",
        type = "tar.gz",
        sha256 = "0e25ea47919b1560c4e3b7fe0aaab9becf5b84a10325ddf7db0f0ba5e1026499",
        strip_prefix = "diff-0.1.12",
        build_file = Label("//third_party/remote:BUILD.diff-0.1.12.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__difference__2_0_0",
        url = "https://crates.io/api/v1/crates/difference/2.0.0/download",
        type = "tar.gz",
        sha256 = "524cbf6897b527295dff137cec09ecf3a05f4fddffd7dfcd1585403449e74198",
        strip_prefix = "difference-2.0.0",
        build_file = Label("//third_party/remote:BUILD.difference-2.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__digest__0_8_1",
        url = "https://crates.io/api/v1/crates/digest/0.8.1/download",
        type = "tar.gz",
        sha256 = "f3d0c8c8752312f9713efd397ff63acb9f85585afbf179282e720e7704954dd5",
        strip_prefix = "digest-0.8.1",
        build_file = Label("//third_party/remote:BUILD.digest-0.8.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__dirs__2_0_2",
        url = "https://crates.io/api/v1/crates/dirs/2.0.2/download",
        type = "tar.gz",
        sha256 = "13aea89a5c93364a98e9b37b2fa237effbb694d5cfe01c5b70941f7eb087d5e3",
        strip_prefix = "dirs-2.0.2",
        build_file = Label("//third_party/remote:BUILD.dirs-2.0.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__dirs_sys__0_3_5",
        url = "https://crates.io/api/v1/crates/dirs-sys/0.3.5/download",
        type = "tar.gz",
        sha256 = "8e93d7f5705de3e49895a2b5e0b8855a1c27f080192ae9c32a6432d50741a57a",
        strip_prefix = "dirs-sys-0.3.5",
        build_file = Label("//third_party/remote:BUILD.dirs-sys-0.3.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__either__1_6_1",
        url = "https://crates.io/api/v1/crates/either/1.6.1/download",
        type = "tar.gz",
        sha256 = "e78d4f1cc4ae33bbfc157ed5d5a5ef3bc29227303d595861deb238fcec4e9457",
        strip_prefix = "either-1.6.1",
        build_file = Label("//third_party/remote:BUILD.either-1.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ena__0_14_0",
        url = "https://crates.io/api/v1/crates/ena/0.14.0/download",
        type = "tar.gz",
        sha256 = "d7402b94a93c24e742487327a7cd839dc9d36fec9de9fb25b09f2dae459f36c3",
        strip_prefix = "ena-0.14.0",
        build_file = Label("//third_party/remote:BUILD.ena-0.14.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__env_logger__0_6_2",
        url = "https://crates.io/api/v1/crates/env_logger/0.6.2/download",
        type = "tar.gz",
        sha256 = "aafcde04e90a5226a6443b7aabdb016ba2f8307c847d524724bd9b346dd1a2d3",
        strip_prefix = "env_logger-0.6.2",
        build_file = Label("//third_party/remote:BUILD.env_logger-0.6.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__event_listener__2_5_1",
        url = "https://crates.io/api/v1/crates/event-listener/2.5.1/download",
        type = "tar.gz",
        sha256 = "f7531096570974c3a9dcf9e4b8e1cede1ec26cf5046219fb3b9d897503b9be59",
        strip_prefix = "event-listener-2.5.1",
        build_file = Label("//third_party/remote:BUILD.event-listener-2.5.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__fake_simd__0_1_2",
        url = "https://crates.io/api/v1/crates/fake-simd/0.1.2/download",
        type = "tar.gz",
        sha256 = "e88a8acf291dafb59c2d96e8f59828f3838bb1a70398823ade51a84de6a6deed",
        strip_prefix = "fake-simd-0.1.2",
        build_file = Label("//third_party/remote:BUILD.fake-simd-0.1.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__fixed_buffer__0_2_2",
        url = "https://crates.io/api/v1/crates/fixed-buffer/0.2.2/download",
        type = "tar.gz",
        sha256 = "4244e6bc495db6e875b4b3c9982b4bd3d69268cfa619273ae047d76d761a755c",
        strip_prefix = "fixed-buffer-0.2.2",
        build_file = Label("//third_party/remote:BUILD.fixed-buffer-0.2.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__fixedbitset__0_2_0",
        url = "https://crates.io/api/v1/crates/fixedbitset/0.2.0/download",
        type = "tar.gz",
        sha256 = "37ab347416e802de484e4d03c7316c48f1ecb56574dfd4a46a80f173ce1de04d",
        strip_prefix = "fixedbitset-0.2.0",
        build_file = Label("//third_party/remote:BUILD.fixedbitset-0.2.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__fnv__1_0_7",
        url = "https://crates.io/api/v1/crates/fnv/1.0.7/download",
        type = "tar.gz",
        sha256 = "3f9eec918d3f24069decb9af1554cad7c880e2da24a9afd88aca000531ab82c1",
        strip_prefix = "fnv-1.0.7",
        build_file = Label("//third_party/remote:BUILD.fnv-1.0.7.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__fuchsia_zircon__0_3_3",
        url = "https://crates.io/api/v1/crates/fuchsia-zircon/0.3.3/download",
        type = "tar.gz",
        sha256 = "2e9763c69ebaae630ba35f74888db465e49e259ba1bc0eda7d06f4a067615d82",
        strip_prefix = "fuchsia-zircon-0.3.3",
        build_file = Label("//third_party/remote:BUILD.fuchsia-zircon-0.3.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__fuchsia_zircon_sys__0_3_3",
        url = "https://crates.io/api/v1/crates/fuchsia-zircon-sys/0.3.3/download",
        type = "tar.gz",
        sha256 = "3dcaa9ae7725d12cdb85b3ad99a434db70b468c09ded17e012d86b5c1010f7a7",
        strip_prefix = "fuchsia-zircon-sys-0.3.3",
        build_file = Label("//third_party/remote:BUILD.fuchsia-zircon-sys-0.3.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures__0_3_8",
        url = "https://crates.io/api/v1/crates/futures/0.3.8/download",
        type = "tar.gz",
        sha256 = "9b3b0c040a1fe6529d30b3c5944b280c7f0dcb2930d2c3062bca967b602583d0",
        strip_prefix = "futures-0.3.8",
        build_file = Label("//third_party/remote:BUILD.futures-0.3.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_channel__0_3_8",
        url = "https://crates.io/api/v1/crates/futures-channel/0.3.8/download",
        type = "tar.gz",
        sha256 = "4b7109687aa4e177ef6fe84553af6280ef2778bdb7783ba44c9dc3399110fe64",
        strip_prefix = "futures-channel-0.3.8",
        build_file = Label("//third_party/remote:BUILD.futures-channel-0.3.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_core__0_3_8",
        url = "https://crates.io/api/v1/crates/futures-core/0.3.8/download",
        type = "tar.gz",
        sha256 = "847ce131b72ffb13b6109a221da9ad97a64cbe48feb1028356b836b47b8f1748",
        strip_prefix = "futures-core-0.3.8",
        build_file = Label("//third_party/remote:BUILD.futures-core-0.3.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_executor__0_3_8",
        url = "https://crates.io/api/v1/crates/futures-executor/0.3.8/download",
        type = "tar.gz",
        sha256 = "4caa2b2b68b880003057c1dd49f1ed937e38f22fcf6c212188a121f08cf40a65",
        strip_prefix = "futures-executor-0.3.8",
        build_file = Label("//third_party/remote:BUILD.futures-executor-0.3.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_io__0_3_8",
        url = "https://crates.io/api/v1/crates/futures-io/0.3.8/download",
        type = "tar.gz",
        sha256 = "611834ce18aaa1bd13c4b374f5d653e1027cf99b6b502584ff8c9a64413b30bb",
        strip_prefix = "futures-io-0.3.8",
        build_file = Label("//third_party/remote:BUILD.futures-io-0.3.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_macro__0_3_8",
        url = "https://crates.io/api/v1/crates/futures-macro/0.3.8/download",
        type = "tar.gz",
        sha256 = "77408a692f1f97bcc61dc001d752e00643408fbc922e4d634c655df50d595556",
        strip_prefix = "futures-macro-0.3.8",
        build_file = Label("//third_party/remote:BUILD.futures-macro-0.3.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_sink__0_3_8",
        url = "https://crates.io/api/v1/crates/futures-sink/0.3.8/download",
        type = "tar.gz",
        sha256 = "f878195a49cee50e006b02b93cf7e0a95a38ac7b776b4c4d9cc1207cd20fcb3d",
        strip_prefix = "futures-sink-0.3.8",
        build_file = Label("//third_party/remote:BUILD.futures-sink-0.3.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_task__0_3_8",
        url = "https://crates.io/api/v1/crates/futures-task/0.3.8/download",
        type = "tar.gz",
        sha256 = "7c554eb5bf48b2426c4771ab68c6b14468b6e76cc90996f528c3338d761a4d0d",
        strip_prefix = "futures-task-0.3.8",
        build_file = Label("//third_party/remote:BUILD.futures-task-0.3.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_util__0_3_8",
        url = "https://crates.io/api/v1/crates/futures-util/0.3.8/download",
        type = "tar.gz",
        sha256 = "d304cff4a7b99cfb7986f7d43fbe93d175e72e704a8860787cc95e9ffd85cbd2",
        strip_prefix = "futures-util-0.3.8",
        build_file = Label("//third_party/remote:BUILD.futures-util-0.3.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__generic_array__0_12_3",
        url = "https://crates.io/api/v1/crates/generic-array/0.12.3/download",
        type = "tar.gz",
        sha256 = "c68f0274ae0e023facc3c97b2e00f076be70e254bc851d972503b328db79b2ec",
        strip_prefix = "generic-array-0.12.3",
        build_file = Label("//third_party/remote:BUILD.generic-array-0.12.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__getopts__0_2_21",
        url = "https://crates.io/api/v1/crates/getopts/0.2.21/download",
        type = "tar.gz",
        sha256 = "14dbbfd5c71d70241ecf9e6f13737f7b5ce823821063188d7e46c41d371eebd5",
        strip_prefix = "getopts-0.2.21",
        build_file = Label("//third_party/remote:BUILD.getopts-0.2.21.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__getrandom__0_1_16",
        url = "https://crates.io/api/v1/crates/getrandom/0.1.16/download",
        type = "tar.gz",
        sha256 = "8fc3cb4d91f53b50155bdcfd23f6a4c39ae1969c2ae85982b135750cccaf5fce",
        strip_prefix = "getrandom-0.1.16",
        build_file = Label("//third_party/remote:BUILD.getrandom-0.1.16.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__globset__0_4_6",
        url = "https://crates.io/api/v1/crates/globset/0.4.6/download",
        type = "tar.gz",
        sha256 = "c152169ef1e421390738366d2f796655fec62621dabbd0fd476f905934061e4a",
        strip_prefix = "globset-0.4.6",
        build_file = Label("//third_party/remote:BUILD.globset-0.4.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__h2__0_2_7",
        url = "https://crates.io/api/v1/crates/h2/0.2.7/download",
        type = "tar.gz",
        sha256 = "5e4728fd124914ad25e99e3d15a9361a879f6620f63cb56bbb08f95abb97a535",
        strip_prefix = "h2-0.2.7",
        build_file = Label("//third_party/remote:BUILD.h2-0.2.7.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hashbrown__0_9_1",
        url = "https://crates.io/api/v1/crates/hashbrown/0.9.1/download",
        type = "tar.gz",
        sha256 = "d7afe4a420e3fe79967a00898cc1f4db7c8a49a9333a29f8a4bd76a253d5cd04",
        strip_prefix = "hashbrown-0.9.1",
        build_file = Label("//third_party/remote:BUILD.hashbrown-0.9.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__heck__0_3_2",
        url = "https://crates.io/api/v1/crates/heck/0.3.2/download",
        type = "tar.gz",
        sha256 = "87cbf45460356b7deeb5e3415b5563308c0a9b057c85e12b06ad551f98d0a6ac",
        strip_prefix = "heck-0.3.2",
        build_file = Label("//third_party/remote:BUILD.heck-0.3.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hermit_abi__0_1_17",
        url = "https://crates.io/api/v1/crates/hermit-abi/0.1.17/download",
        type = "tar.gz",
        sha256 = "5aca5565f760fb5b220e499d72710ed156fdb74e631659e99377d9ebfbd13ae8",
        strip_prefix = "hermit-abi-0.1.17",
        build_file = Label("//third_party/remote:BUILD.hermit-abi-0.1.17.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hex__0_4_2",
        url = "https://crates.io/api/v1/crates/hex/0.4.2/download",
        type = "tar.gz",
        sha256 = "644f9158b2f133fd50f5fb3242878846d9eb792e445c893805ff0e3824006e35",
        strip_prefix = "hex-0.4.2",
        build_file = Label("//third_party/remote:BUILD.hex-0.4.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__http__0_2_2",
        url = "https://crates.io/api/v1/crates/http/0.2.2/download",
        type = "tar.gz",
        sha256 = "84129d298a6d57d246960ff8eb831ca4af3f96d29e2e28848dae275408658e26",
        strip_prefix = "http-0.2.2",
        build_file = Label("//third_party/remote:BUILD.http-0.2.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__http_body__0_3_1",
        url = "https://crates.io/api/v1/crates/http-body/0.3.1/download",
        type = "tar.gz",
        sha256 = "13d5ff830006f7646652e057693569bfe0d51760c0085a071769d142a205111b",
        strip_prefix = "http-body-0.3.1",
        build_file = Label("//third_party/remote:BUILD.http-body-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__httparse__1_3_4",
        url = "https://crates.io/api/v1/crates/httparse/1.3.4/download",
        type = "tar.gz",
        sha256 = "cd179ae861f0c2e53da70d892f5f3029f9594be0c41dc5269cd371691b1dc2f9",
        strip_prefix = "httparse-1.3.4",
        build_file = Label("//third_party/remote:BUILD.httparse-1.3.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__httpdate__0_3_2",
        url = "https://crates.io/api/v1/crates/httpdate/0.3.2/download",
        type = "tar.gz",
        sha256 = "494b4d60369511e7dea41cf646832512a94e542f68bb9c49e54518e0f468eb47",
        strip_prefix = "httpdate-0.3.2",
        build_file = Label("//third_party/remote:BUILD.httpdate-0.3.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__humantime__1_3_0",
        url = "https://crates.io/api/v1/crates/humantime/1.3.0/download",
        type = "tar.gz",
        sha256 = "df004cfca50ef23c36850aaaa59ad52cc70d0e90243c3c7737a4dd32dc7a3c4f",
        strip_prefix = "humantime-1.3.0",
        build_file = Label("//third_party/remote:BUILD.humantime-1.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hyper__0_13_9",
        url = "https://crates.io/api/v1/crates/hyper/0.13.9/download",
        type = "tar.gz",
        sha256 = "f6ad767baac13b44d4529fcf58ba2cd0995e36e7b435bc5b039de6f47e880dbf",
        strip_prefix = "hyper-0.13.9",
        build_file = Label("//third_party/remote:BUILD.hyper-0.13.9.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ignore__0_4_17",
        url = "https://crates.io/api/v1/crates/ignore/0.4.17/download",
        type = "tar.gz",
        sha256 = "b287fb45c60bb826a0dc68ff08742b9d88a2fea13d6e0c286b3172065aaf878c",
        strip_prefix = "ignore-0.4.17",
        build_file = Label("//third_party/remote:BUILD.ignore-0.4.17.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__indexmap__1_6_1",
        url = "https://crates.io/api/v1/crates/indexmap/1.6.1/download",
        type = "tar.gz",
        sha256 = "4fb1fa934250de4de8aef298d81c729a7d33d8c239daa3a7575e6b92bfc7313b",
        strip_prefix = "indexmap-1.6.1",
        build_file = Label("//third_party/remote:BUILD.indexmap-1.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__iovec__0_1_4",
        url = "https://crates.io/api/v1/crates/iovec/0.1.4/download",
        type = "tar.gz",
        sha256 = "b2b3ea6ff95e175473f8ffe6a7eb7c00d054240321b84c57051175fe3c1e075e",
        strip_prefix = "iovec-0.1.4",
        build_file = Label("//third_party/remote:BUILD.iovec-0.1.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__itertools__0_8_2",
        url = "https://crates.io/api/v1/crates/itertools/0.8.2/download",
        type = "tar.gz",
        sha256 = "f56a2d0bc861f9165be4eb3442afd3c236d8a98afd426f65d92324ae1091a484",
        strip_prefix = "itertools-0.8.2",
        build_file = Label("//third_party/remote:BUILD.itertools-0.8.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__itoa__0_4_7",
        url = "https://crates.io/api/v1/crates/itoa/0.4.7/download",
        type = "tar.gz",
        sha256 = "dd25036021b0de88a0aff6b850051563c6516d0bf53f8638938edbb9de732736",
        strip_prefix = "itoa-0.4.7",
        build_file = Label("//third_party/remote:BUILD.itoa-0.4.7.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__jobserver__0_1_21",
        url = "https://crates.io/api/v1/crates/jobserver/0.1.21/download",
        type = "tar.gz",
        sha256 = "5c71313ebb9439f74b00d9d2dcec36440beaf57a6aa0623068441dd7cd81a7f2",
        strip_prefix = "jobserver-0.1.21",
        build_file = Label("//third_party/remote:BUILD.jobserver-0.1.21.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__kernel32_sys__0_2_2",
        url = "https://crates.io/api/v1/crates/kernel32-sys/0.2.2/download",
        type = "tar.gz",
        sha256 = "7507624b29483431c0ba2d82aece8ca6cdba9382bff4ddd0f7490560c056098d",
        strip_prefix = "kernel32-sys-0.2.2",
        build_file = Label("//third_party/remote:BUILD.kernel32-sys-0.2.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__lazy_static__1_4_0",
        url = "https://crates.io/api/v1/crates/lazy_static/1.4.0/download",
        type = "tar.gz",
        sha256 = "e2abad23fbc42b3700f2f279844dc832adb2b2eb069b2df918f455c4e18cc646",
        strip_prefix = "lazy_static-1.4.0",
        build_file = Label("//third_party/remote:BUILD.lazy_static-1.4.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__libc__0_2_81",
        url = "https://crates.io/api/v1/crates/libc/0.2.81/download",
        type = "tar.gz",
        sha256 = "1482821306169ec4d07f6aca392a4681f66c75c9918aa49641a2595db64053cb",
        strip_prefix = "libc-0.2.81",
        build_file = Label("//third_party/remote:BUILD.libc-0.2.81.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__lock_api__0_3_4",
        url = "https://crates.io/api/v1/crates/lock_api/0.3.4/download",
        type = "tar.gz",
        sha256 = "c4da24a77a3d8a6d4862d95f72e6fdb9c09a643ecdb402d754004a557f2bec75",
        strip_prefix = "lock_api-0.3.4",
        build_file = Label("//third_party/remote:BUILD.lock_api-0.3.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__log__0_4_11",
        url = "https://crates.io/api/v1/crates/log/0.4.11/download",
        type = "tar.gz",
        sha256 = "4fabed175da42fed1fa0746b0ea71f412aa9d35e76e95e59b192c64b9dc2bf8b",
        strip_prefix = "log-0.4.11",
        build_file = Label("//third_party/remote:BUILD.log-0.4.11.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__maybe_uninit__2_0_0",
        url = "https://crates.io/api/v1/crates/maybe-uninit/2.0.0/download",
        type = "tar.gz",
        sha256 = "60302e4db3a61da70c0cb7991976248362f30319e88850c487b9b95bbf059e00",
        strip_prefix = "maybe-uninit-2.0.0",
        build_file = Label("//third_party/remote:BUILD.maybe-uninit-2.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__md_5__0_8_0",
        url = "https://crates.io/api/v1/crates/md-5/0.8.0/download",
        type = "tar.gz",
        sha256 = "a18af3dcaf2b0219366cdb4e2af65a6101457b415c3d1a5c71dd9c2b7c77b9c8",
        strip_prefix = "md-5-0.8.0",
        build_file = Label("//third_party/remote:BUILD.md-5-0.8.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__measureme__0_7_1",
        url = "https://crates.io/api/v1/crates/measureme/0.7.1/download",
        type = "tar.gz",
        sha256 = "fef709d3257013bba7cff14fc504e07e80631d3fe0f6d38ce63b8f6510ccb932",
        strip_prefix = "measureme-0.7.1",
        build_file = Label("//third_party/remote:BUILD.measureme-0.7.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__memchr__2_3_4",
        url = "https://crates.io/api/v1/crates/memchr/2.3.4/download",
        type = "tar.gz",
        sha256 = "0ee1c47aaa256ecabcaea351eae4a9b01ef39ed810004e298d2511ed284b1525",
        strip_prefix = "memchr-2.3.4",
        build_file = Label("//third_party/remote:BUILD.memchr-2.3.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__memmap__0_7_0",
        url = "https://crates.io/api/v1/crates/memmap/0.7.0/download",
        type = "tar.gz",
        sha256 = "6585fd95e7bb50d6cc31e20d4cf9afb4e2ba16c5846fc76793f11218da9c475b",
        strip_prefix = "memmap-0.7.0",
        build_file = Label("//third_party/remote:BUILD.memmap-0.7.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__memoffset__0_5_6",
        url = "https://crates.io/api/v1/crates/memoffset/0.5.6/download",
        type = "tar.gz",
        sha256 = "043175f069eda7b85febe4a74abbaeff828d9f8b448515d3151a14a3542811aa",
        strip_prefix = "memoffset-0.5.6",
        build_file = Label("//third_party/remote:BUILD.memoffset-0.5.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__mio__0_6_23",
        url = "https://crates.io/api/v1/crates/mio/0.6.23/download",
        type = "tar.gz",
        sha256 = "4afd66f5b91bf2a3bc13fad0e21caedac168ca4c707504e75585648ae80e4cc4",
        strip_prefix = "mio-0.6.23",
        build_file = Label("//third_party/remote:BUILD.mio-0.6.23.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__miow__0_2_2",
        url = "https://crates.io/api/v1/crates/miow/0.2.2/download",
        type = "tar.gz",
        sha256 = "ebd808424166322d4a38da87083bfddd3ac4c131334ed55856112eb06d46944d",
        strip_prefix = "miow-0.2.2",
        build_file = Label("//third_party/remote:BUILD.miow-0.2.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__multimap__0_8_2",
        url = "https://crates.io/api/v1/crates/multimap/0.8.2/download",
        type = "tar.gz",
        sha256 = "1255076139a83bb467426e7f8d0134968a8118844faa755985e077cf31850333",
        strip_prefix = "multimap-0.8.2",
        build_file = Label("//third_party/remote:BUILD.multimap-0.8.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__net2__0_2_37",
        url = "https://crates.io/api/v1/crates/net2/0.2.37/download",
        type = "tar.gz",
        sha256 = "391630d12b68002ae1e25e8f974306474966550ad82dac6886fb8910c19568ae",
        strip_prefix = "net2-0.2.37",
        build_file = Label("//third_party/remote:BUILD.net2-0.2.37.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__num_cpus__1_13_0",
        url = "https://crates.io/api/v1/crates/num_cpus/1.13.0/download",
        type = "tar.gz",
        sha256 = "05499f3756671c15885fee9034446956fff3f243d6077b91e5767df161f766b3",
        strip_prefix = "num_cpus-1.13.0",
        build_file = Label("//third_party/remote:BUILD.num_cpus-1.13.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__once_cell__1_5_2",
        url = "https://crates.io/api/v1/crates/once_cell/1.5.2/download",
        type = "tar.gz",
        sha256 = "13bd41f508810a131401606d54ac32a467c97172d74ba7662562ebba5ad07fa0",
        strip_prefix = "once_cell-1.5.2",
        build_file = Label("//third_party/remote:BUILD.once_cell-1.5.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__opaque_debug__0_2_3",
        url = "https://crates.io/api/v1/crates/opaque-debug/0.2.3/download",
        type = "tar.gz",
        sha256 = "2839e79665f131bdb5782e51f2c6c9599c133c6098982a54c794358bf432529c",
        strip_prefix = "opaque-debug-0.2.3",
        build_file = Label("//third_party/remote:BUILD.opaque-debug-0.2.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__output_vt100__0_1_2",
        url = "https://crates.io/api/v1/crates/output_vt100/0.1.2/download",
        type = "tar.gz",
        sha256 = "53cdc5b785b7a58c5aad8216b3dfa114df64b0b06ae6e1501cef91df2fbdf8f9",
        strip_prefix = "output_vt100-0.1.2",
        build_file = Label("//third_party/remote:BUILD.output_vt100-0.1.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__parking_lot__0_10_2",
        url = "https://crates.io/api/v1/crates/parking_lot/0.10.2/download",
        type = "tar.gz",
        sha256 = "d3a704eb390aafdc107b0e392f56a82b668e3a71366993b5340f5833fd62505e",
        strip_prefix = "parking_lot-0.10.2",
        build_file = Label("//third_party/remote:BUILD.parking_lot-0.10.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__parking_lot__0_9_0",
        url = "https://crates.io/api/v1/crates/parking_lot/0.9.0/download",
        type = "tar.gz",
        sha256 = "f842b1982eb6c2fe34036a4fbfb06dd185a3f5c8edfaacdf7d1ea10b07de6252",
        strip_prefix = "parking_lot-0.9.0",
        build_file = Label("//third_party/remote:BUILD.parking_lot-0.9.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__parking_lot_core__0_6_2",
        url = "https://crates.io/api/v1/crates/parking_lot_core/0.6.2/download",
        type = "tar.gz",
        sha256 = "b876b1b9e7ac6e1a74a6da34d25c42e17e8862aa409cbbbdcfc8d86c6f3bc62b",
        strip_prefix = "parking_lot_core-0.6.2",
        build_file = Label("//third_party/remote:BUILD.parking_lot_core-0.6.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__parking_lot_core__0_7_2",
        url = "https://crates.io/api/v1/crates/parking_lot_core/0.7.2/download",
        type = "tar.gz",
        sha256 = "d58c7c768d4ba344e3e8d72518ac13e259d7c7ade24167003b8488e10b6740a3",
        strip_prefix = "parking_lot_core-0.7.2",
        build_file = Label("//third_party/remote:BUILD.parking_lot_core-0.7.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__percent_encoding__2_1_0",
        url = "https://crates.io/api/v1/crates/percent-encoding/2.1.0/download",
        type = "tar.gz",
        sha256 = "d4fd5641d01c8f18a23da7b6fe29298ff4b55afcccdf78973b24cf3175fee32e",
        strip_prefix = "percent-encoding-2.1.0",
        build_file = Label("//third_party/remote:BUILD.percent-encoding-2.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__petgraph__0_5_1",
        url = "https://crates.io/api/v1/crates/petgraph/0.5.1/download",
        type = "tar.gz",
        sha256 = "467d164a6de56270bd7c4d070df81d07beace25012d5103ced4e9ff08d6afdb7",
        strip_prefix = "petgraph-0.5.1",
        build_file = Label("//third_party/remote:BUILD.petgraph-0.5.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pin_project__0_4_27",
        url = "https://crates.io/api/v1/crates/pin-project/0.4.27/download",
        type = "tar.gz",
        sha256 = "2ffbc8e94b38ea3d2d8ba92aea2983b503cd75d0888d75b86bb37970b5698e15",
        strip_prefix = "pin-project-0.4.27",
        build_file = Label("//third_party/remote:BUILD.pin-project-0.4.27.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pin_project__1_0_2",
        url = "https://crates.io/api/v1/crates/pin-project/1.0.2/download",
        type = "tar.gz",
        sha256 = "9ccc2237c2c489783abd8c4c80e5450fc0e98644555b1364da68cc29aa151ca7",
        strip_prefix = "pin-project-1.0.2",
        build_file = Label("//third_party/remote:BUILD.pin-project-1.0.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pin_project_internal__0_4_27",
        url = "https://crates.io/api/v1/crates/pin-project-internal/0.4.27/download",
        type = "tar.gz",
        sha256 = "65ad2ae56b6abe3a1ee25f15ee605bacadb9a764edaba9c2bf4103800d4a1895",
        strip_prefix = "pin-project-internal-0.4.27",
        build_file = Label("//third_party/remote:BUILD.pin-project-internal-0.4.27.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pin_project_internal__1_0_2",
        url = "https://crates.io/api/v1/crates/pin-project-internal/1.0.2/download",
        type = "tar.gz",
        sha256 = "f8e8d2bf0b23038a4424865103a4df472855692821aab4e4f5c3312d461d9e5f",
        strip_prefix = "pin-project-internal-1.0.2",
        build_file = Label("//third_party/remote:BUILD.pin-project-internal-1.0.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pin_project_lite__0_1_11",
        url = "https://crates.io/api/v1/crates/pin-project-lite/0.1.11/download",
        type = "tar.gz",
        sha256 = "c917123afa01924fc84bb20c4c03f004d9c38e5127e3c039bbf7f4b9c76a2f6b",
        strip_prefix = "pin-project-lite-0.1.11",
        build_file = Label("//third_party/remote:BUILD.pin-project-lite-0.1.11.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pin_project_lite__0_2_0",
        url = "https://crates.io/api/v1/crates/pin-project-lite/0.2.0/download",
        type = "tar.gz",
        sha256 = "6b063f57ec186e6140e2b8b6921e5f1bd89c7356dda5b33acc5401203ca6131c",
        strip_prefix = "pin-project-lite-0.2.0",
        build_file = Label("//third_party/remote:BUILD.pin-project-lite-0.2.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pin_utils__0_1_0",
        url = "https://crates.io/api/v1/crates/pin-utils/0.1.0/download",
        type = "tar.gz",
        sha256 = "8b870d8c151b6f2fb93e84a13146138f05d02ed11c7e7c54f8826aaaf7c9f184",
        strip_prefix = "pin-utils-0.1.0",
        build_file = Label("//third_party/remote:BUILD.pin-utils-0.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ppv_lite86__0_2_10",
        url = "https://crates.io/api/v1/crates/ppv-lite86/0.2.10/download",
        type = "tar.gz",
        sha256 = "ac74c624d6b2d21f425f752262f42188365d7b8ff1aff74c82e45136510a4857",
        strip_prefix = "ppv-lite86-0.2.10",
        build_file = Label("//third_party/remote:BUILD.ppv-lite86-0.2.10.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pretty_assertions__0_6_1",
        url = "https://crates.io/api/v1/crates/pretty_assertions/0.6.1/download",
        type = "tar.gz",
        sha256 = "3f81e1644e1b54f5a68959a29aa86cde704219254669da328ecfdf6a1f09d427",
        strip_prefix = "pretty_assertions-0.6.1",
        build_file = Label("//third_party/remote:BUILD.pretty_assertions-0.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__proc_macro_error__1_0_4",
        url = "https://crates.io/api/v1/crates/proc-macro-error/1.0.4/download",
        type = "tar.gz",
        sha256 = "da25490ff9892aab3fcf7c36f08cfb902dd3e71ca0f9f9517bea02a73a5ce38c",
        strip_prefix = "proc-macro-error-1.0.4",
        build_file = Label("//third_party/remote:BUILD.proc-macro-error-1.0.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__proc_macro_error_attr__1_0_4",
        url = "https://crates.io/api/v1/crates/proc-macro-error-attr/1.0.4/download",
        type = "tar.gz",
        sha256 = "a1be40180e52ecc98ad80b184934baf3d0d29f979574e439af5a55274b35f869",
        strip_prefix = "proc-macro-error-attr-1.0.4",
        build_file = Label("//third_party/remote:BUILD.proc-macro-error-attr-1.0.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__proc_macro_hack__0_5_19",
        url = "https://crates.io/api/v1/crates/proc-macro-hack/0.5.19/download",
        type = "tar.gz",
        sha256 = "dbf0c48bc1d91375ae5c3cd81e3722dff1abcf81a30960240640d223f59fe0e5",
        strip_prefix = "proc-macro-hack-0.5.19",
        build_file = Label("//third_party/remote:BUILD.proc-macro-hack-0.5.19.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__proc_macro_nested__0_1_6",
        url = "https://crates.io/api/v1/crates/proc-macro-nested/0.1.6/download",
        type = "tar.gz",
        sha256 = "eba180dafb9038b050a4c280019bbedf9f2467b61e5d892dcad585bb57aadc5a",
        strip_prefix = "proc-macro-nested-0.1.6",
        build_file = Label("//third_party/remote:BUILD.proc-macro-nested-0.1.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__proc_macro2__1_0_24",
        url = "https://crates.io/api/v1/crates/proc-macro2/1.0.24/download",
        type = "tar.gz",
        sha256 = "1e0704ee1a7e00d7bb417d0770ea303c1bccbabf0ef1667dae92b5967f5f8a71",
        strip_prefix = "proc-macro2-1.0.24",
        build_file = Label("//third_party/remote:BUILD.proc-macro2-1.0.24.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__prost__0_6_1",
        url = "https://crates.io/api/v1/crates/prost/0.6.1/download",
        type = "tar.gz",
        sha256 = "ce49aefe0a6144a45de32927c77bd2859a5f7677b55f220ae5b744e87389c212",
        strip_prefix = "prost-0.6.1",
        build_file = Label("//third_party/remote:BUILD.prost-0.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__prost_build__0_6_1",
        url = "https://crates.io/api/v1/crates/prost-build/0.6.1/download",
        type = "tar.gz",
        sha256 = "02b10678c913ecbd69350e8535c3aef91a8676c0773fc1d7b95cdd196d7f2f26",
        strip_prefix = "prost-build-0.6.1",
        build_file = Label("//third_party/remote:BUILD.prost-build-0.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__prost_derive__0_6_1",
        url = "https://crates.io/api/v1/crates/prost-derive/0.6.1/download",
        type = "tar.gz",
        sha256 = "537aa19b95acde10a12fec4301466386f757403de4cd4e5b4fa78fb5ecb18f72",
        strip_prefix = "prost-derive-0.6.1",
        build_file = Label("//third_party/remote:BUILD.prost-derive-0.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__prost_types__0_6_1",
        url = "https://crates.io/api/v1/crates/prost-types/0.6.1/download",
        type = "tar.gz",
        sha256 = "1834f67c0697c001304b75be76f67add9c89742eda3a085ad8ee0bb38c3417aa",
        strip_prefix = "prost-types-0.6.1",
        build_file = Label("//third_party/remote:BUILD.prost-types-0.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__psm__0_1_12",
        url = "https://crates.io/api/v1/crates/psm/0.1.12/download",
        type = "tar.gz",
        sha256 = "3abf49e5417290756acfd26501536358560c4a5cc4a0934d390939acb3e7083a",
        strip_prefix = "psm-0.1.12",
        build_file = Label("//third_party/remote:BUILD.psm-0.1.12.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__quick_error__1_2_3",
        url = "https://crates.io/api/v1/crates/quick-error/1.2.3/download",
        type = "tar.gz",
        sha256 = "a1d01941d82fa2ab50be1e79e6714289dd7cde78eba4c074bc5a4374f650dfe0",
        strip_prefix = "quick-error-1.2.3",
        build_file = Label("//third_party/remote:BUILD.quick-error-1.2.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__quote__1_0_8",
        url = "https://crates.io/api/v1/crates/quote/1.0.8/download",
        type = "tar.gz",
        sha256 = "991431c3519a3f36861882da93630ce66b52918dcf1b8e2fd66b397fc96f28df",
        strip_prefix = "quote-1.0.8",
        build_file = Label("//third_party/remote:BUILD.quote-1.0.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rand__0_7_3",
        url = "https://crates.io/api/v1/crates/rand/0.7.3/download",
        type = "tar.gz",
        sha256 = "6a6b1679d49b24bbfe0c803429aa1874472f50d9b363131f0e89fc356b544d03",
        strip_prefix = "rand-0.7.3",
        build_file = Label("//third_party/remote:BUILD.rand-0.7.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rand_chacha__0_2_2",
        url = "https://crates.io/api/v1/crates/rand_chacha/0.2.2/download",
        type = "tar.gz",
        sha256 = "f4c8ed856279c9737206bf725bf36935d8666ead7aa69b52be55af369d193402",
        strip_prefix = "rand_chacha-0.2.2",
        build_file = Label("//third_party/remote:BUILD.rand_chacha-0.2.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rand_core__0_5_1",
        url = "https://crates.io/api/v1/crates/rand_core/0.5.1/download",
        type = "tar.gz",
        sha256 = "90bde5296fc891b0cef12a6d03ddccc162ce7b2aff54160af9338f8d40df6d19",
        strip_prefix = "rand_core-0.5.1",
        build_file = Label("//third_party/remote:BUILD.rand_core-0.5.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rand_hc__0_2_0",
        url = "https://crates.io/api/v1/crates/rand_hc/0.2.0/download",
        type = "tar.gz",
        sha256 = "ca3129af7b92a17112d59ad498c6f81eaf463253766b90396d39ea7a39d6613c",
        strip_prefix = "rand_hc-0.2.0",
        build_file = Label("//third_party/remote:BUILD.rand_hc-0.2.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rand_pcg__0_2_1",
        url = "https://crates.io/api/v1/crates/rand_pcg/0.2.1/download",
        type = "tar.gz",
        sha256 = "16abd0c1b639e9eb4d7c50c0b8100b0d0f849be2349829c740fe8e6eb4816429",
        strip_prefix = "rand_pcg-0.2.1",
        build_file = Label("//third_party/remote:BUILD.rand_pcg-0.2.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__redox_syscall__0_1_57",
        url = "https://crates.io/api/v1/crates/redox_syscall/0.1.57/download",
        type = "tar.gz",
        sha256 = "41cc0f7e4d5d4544e8861606a285bb08d3e70712ccc7d2b84d7c0ccfaf4b05ce",
        strip_prefix = "redox_syscall-0.1.57",
        build_file = Label("//third_party/remote:BUILD.redox_syscall-0.1.57.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__redox_users__0_3_5",
        url = "https://crates.io/api/v1/crates/redox_users/0.3.5/download",
        type = "tar.gz",
        sha256 = "de0737333e7a9502c789a36d7c7fa6092a49895d4faa31ca5df163857ded2e9d",
        strip_prefix = "redox_users-0.3.5",
        build_file = Label("//third_party/remote:BUILD.redox_users-0.3.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__regex__1_4_2",
        url = "https://crates.io/api/v1/crates/regex/1.4.2/download",
        type = "tar.gz",
        sha256 = "38cf2c13ed4745de91a5eb834e11c00bcc3709e773173b2ce4c56c9fbde04b9c",
        strip_prefix = "regex-1.4.2",
        build_file = Label("//third_party/remote:BUILD.regex-1.4.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__regex_syntax__0_6_21",
        url = "https://crates.io/api/v1/crates/regex-syntax/0.6.21/download",
        type = "tar.gz",
        sha256 = "3b181ba2dcf07aaccad5448e8ead58db5b742cf85dfe035e2227f137a539a189",
        strip_prefix = "regex-syntax-0.6.21",
        build_file = Label("//third_party/remote:BUILD.regex-syntax-0.6.21.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__remove_dir_all__0_5_3",
        url = "https://crates.io/api/v1/crates/remove_dir_all/0.5.3/download",
        type = "tar.gz",
        sha256 = "3acd125665422973a33ac9d3dd2df85edad0f4ae9b00dafb1a05e43a9f5ef8e7",
        strip_prefix = "remove_dir_all-0.5.3",
        build_file = Label("//third_party/remote:BUILD.remove_dir_all-0.5.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rust_argon2__0_8_3",
        url = "https://crates.io/api/v1/crates/rust-argon2/0.8.3/download",
        type = "tar.gz",
        sha256 = "4b18820d944b33caa75a71378964ac46f58517c92b6ae5f762636247c09e78fb",
        strip_prefix = "rust-argon2-0.8.3",
        build_file = Label("//third_party/remote:BUILD.rust-argon2-0.8.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_arena__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_arena/677.0.0/download",
        type = "tar.gz",
        sha256 = "2958af0d6e0458434a25cd3a96f6e19f24f71bf50b900add520dec52e212866b",
        strip_prefix = "rustc-ap-rustc_arena-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_arena-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_ast__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_ast/677.0.0/download",
        type = "tar.gz",
        sha256 = "0c82c2510460f2133548e62399e5acd30c25ae6ece30245baab3d1e00c2fefac",
        strip_prefix = "rustc-ap-rustc_ast-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_ast-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_ast_passes__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_ast_passes/677.0.0/download",
        type = "tar.gz",
        sha256 = "83977da57f81c6edd89bad47e49136680eaa33288de4abb702e95358c2a0fc6c",
        strip_prefix = "rustc-ap-rustc_ast_passes-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_ast_passes-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_ast_pretty__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_ast_pretty/677.0.0/download",
        type = "tar.gz",
        sha256 = "becf4ca1638b214694c71a8752192683048ab8bd47947cc481f57bd48157eeb9",
        strip_prefix = "rustc-ap-rustc_ast_pretty-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_ast_pretty-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_attr__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_attr/677.0.0/download",
        type = "tar.gz",
        sha256 = "0f21ca5dadce8a40d75a2756b77eab75b4c2d827f645c622dd93ee2285599640",
        strip_prefix = "rustc-ap-rustc_attr-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_attr-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_data_structures__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_data_structures/677.0.0/download",
        type = "tar.gz",
        sha256 = "a4cd204764727fde9abf75333eb661f058bfc7242062d91019440fe1b240688b",
        strip_prefix = "rustc-ap-rustc_data_structures-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_data_structures-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_errors__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_errors/677.0.0/download",
        type = "tar.gz",
        sha256 = "58116f119e37f14c029f99077b347069621118e048a69df74695b98204e7c136",
        strip_prefix = "rustc-ap-rustc_errors-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_errors-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_expand__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_expand/677.0.0/download",
        type = "tar.gz",
        sha256 = "48e3c4bda9b64b92805bebe7431fdb8e24fd112b35a8c6d2174827441f10a6b2",
        strip_prefix = "rustc-ap-rustc_expand-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_expand-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_feature__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_feature/677.0.0/download",
        type = "tar.gz",
        sha256 = "4b612bb67d3fc49f395b03fc4ea4384a0145b05afbadab725803074ec827632b",
        strip_prefix = "rustc-ap-rustc_feature-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_feature-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_fs_util__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_fs_util/677.0.0/download",
        type = "tar.gz",
        sha256 = "7630ad1a73a8434ee920676148cb5440ac57509bd20e94ec41087fb0b1d11c28",
        strip_prefix = "rustc-ap-rustc_fs_util-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_fs_util-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_graphviz__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_graphviz/677.0.0/download",
        type = "tar.gz",
        sha256 = "a603fca4817062eb4fb23ff129d475bd66a69fb32f34ed4362ae950cf814b49d",
        strip_prefix = "rustc-ap-rustc_graphviz-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_graphviz-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_index__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_index/677.0.0/download",
        type = "tar.gz",
        sha256 = "9850c4a5d7c341513e10802bca9588bf8f452ceea2d5cfa87b934246a52622bc",
        strip_prefix = "rustc-ap-rustc_index-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_index-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_lexer__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_lexer/677.0.0/download",
        type = "tar.gz",
        sha256 = "6d86722e5a1a615b198327d0d794cd9cbc8b9db4542276fc51fe078924de68ea",
        strip_prefix = "rustc-ap-rustc_lexer-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_lexer-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_macros__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_macros/677.0.0/download",
        type = "tar.gz",
        sha256 = "b3fc8482e44cabdda7ac9a8e224aef62ebdf95274d629dac8db3b42321025fea",
        strip_prefix = "rustc-ap-rustc_macros-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_macros-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_parse__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_parse/677.0.0/download",
        type = "tar.gz",
        sha256 = "3716cdcd978a91dbd4a2788400e90e809527f841426fbeb92f882f9b8582f3ab",
        strip_prefix = "rustc-ap-rustc_parse-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_parse-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_serialize__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_serialize/677.0.0/download",
        type = "tar.gz",
        sha256 = "c68046d07988b349b2e1c8bc1c9664a1d06519354aa677b9df358c5c5c058da0",
        strip_prefix = "rustc-ap-rustc_serialize-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_serialize-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_session__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_session/677.0.0/download",
        type = "tar.gz",
        sha256 = "85735553501a4de0c8904e37b7ccef79cc1c585a7d7f2cfa02cc38e0d149f982",
        strip_prefix = "rustc-ap-rustc_session-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_session-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_span__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_span/677.0.0/download",
        type = "tar.gz",
        sha256 = "1c49ae8a0d3b9e27c6ffe8febeaa30f899294fff012de70625f9ee81c54fda85",
        strip_prefix = "rustc-ap-rustc_span-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_span-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_ap_rustc_target__677_0_0",
        url = "https://crates.io/api/v1/crates/rustc-ap-rustc_target/677.0.0/download",
        type = "tar.gz",
        sha256 = "1765f447594740c501c7b666b87639aa7c1dae2bf8c3166d5d2dca16646fd034",
        strip_prefix = "rustc-ap-rustc_target-677.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-ap-rustc_target-677.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_hash__1_1_0",
        url = "https://crates.io/api/v1/crates/rustc-hash/1.1.0/download",
        type = "tar.gz",
        sha256 = "08d43f7aa6b08d49f382cde6a7982047c3426db949b1424bc4b7ec9ae12c6ce2",
        strip_prefix = "rustc-hash-1.1.0",
        build_file = Label("//third_party/remote:BUILD.rustc-hash-1.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_rayon__0_3_0",
        url = "https://crates.io/api/v1/crates/rustc-rayon/0.3.0/download",
        type = "tar.gz",
        sha256 = "f32767f90d938f1b7199a174ef249ae1924f6e5bbdb9d112fea141e016f25b3a",
        strip_prefix = "rustc-rayon-0.3.0",
        build_file = Label("//third_party/remote:BUILD.rustc-rayon-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_rayon_core__0_3_0",
        url = "https://crates.io/api/v1/crates/rustc-rayon-core/0.3.0/download",
        type = "tar.gz",
        sha256 = "ea2427831f0053ea3ea73559c8eabd893133a51b251d142bacee53c62a288cb3",
        strip_prefix = "rustc-rayon-core-0.3.0",
        build_file = Label("//third_party/remote:BUILD.rustc-rayon-core-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_workspace_hack__1_0_0",
        url = "https://crates.io/api/v1/crates/rustc-workspace-hack/1.0.0/download",
        type = "tar.gz",
        sha256 = "fc71d2faa173b74b232dedc235e3ee1696581bb132fc116fa3626d6151a1a8fb",
        strip_prefix = "rustc-workspace-hack-1.0.0",
        build_file = Label("//third_party/remote:BUILD.rustc-workspace-hack-1.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_version__0_2_3",
        url = "https://crates.io/api/v1/crates/rustc_version/0.2.3/download",
        type = "tar.gz",
        sha256 = "138e3e0acb6c9fb258b19b67cb8abd63c00679d2851805ea151465464fe9030a",
        strip_prefix = "rustc_version-0.2.3",
        build_file = Label("//third_party/remote:BUILD.rustc_version-0.2.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustfmt_config_proc_macro__0_2_0",
        url = "https://crates.io/api/v1/crates/rustfmt-config_proc_macro/0.2.0/download",
        type = "tar.gz",
        sha256 = "b19836fdb238d3f321427a41b87e6c2e9ac132f209d1dc55c55fae8d1df3996f",
        strip_prefix = "rustfmt-config_proc_macro-0.2.0",
        build_file = Label("//third_party/remote:BUILD.rustfmt-config_proc_macro-0.2.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustfmt_nightly__1_4_21",
        url = "https://crates.io/api/v1/crates/rustfmt-nightly/1.4.21/download",
        type = "tar.gz",
        sha256 = "94904255643aa08b7e4d1c29dc648446c918bdbc9c5ffc39de26b6a9131b2b36",
        strip_prefix = "rustfmt-nightly-1.4.21",
        build_file = Label("//third_party/remote:BUILD.rustfmt-nightly-1.4.21.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ryu__1_0_5",
        url = "https://crates.io/api/v1/crates/ryu/1.0.5/download",
        type = "tar.gz",
        sha256 = "71d301d4193d031abdd79ff7e3dd721168a9572ef3fe51a1517aba235bd8f86e",
        strip_prefix = "ryu-1.0.5",
        build_file = Label("//third_party/remote:BUILD.ryu-1.0.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__same_file__1_0_6",
        url = "https://crates.io/api/v1/crates/same-file/1.0.6/download",
        type = "tar.gz",
        sha256 = "93fc1dc3aaa9bfed95e02e6eadabb4baf7e3078b0bd1b4d7b6b0b68378900502",
        strip_prefix = "same-file-1.0.6",
        build_file = Label("//third_party/remote:BUILD.same-file-1.0.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__scoped_tls__1_0_0",
        url = "https://crates.io/api/v1/crates/scoped-tls/1.0.0/download",
        type = "tar.gz",
        sha256 = "ea6a9290e3c9cf0f18145ef7ffa62d68ee0bf5fcd651017e586dc7fd5da448c2",
        strip_prefix = "scoped-tls-1.0.0",
        build_file = Label("//third_party/remote:BUILD.scoped-tls-1.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__scopeguard__1_1_0",
        url = "https://crates.io/api/v1/crates/scopeguard/1.1.0/download",
        type = "tar.gz",
        sha256 = "d29ab0c6d3fc0ee92fe66e2d99f700eab17a8d57d1c1d3b748380fb20baa78cd",
        strip_prefix = "scopeguard-1.1.0",
        build_file = Label("//third_party/remote:BUILD.scopeguard-1.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__semver__0_9_0",
        url = "https://crates.io/api/v1/crates/semver/0.9.0/download",
        type = "tar.gz",
        sha256 = "1d7eb9ef2c18661902cc47e535f9bc51b78acd254da71d375c2f6720d9a40403",
        strip_prefix = "semver-0.9.0",
        build_file = Label("//third_party/remote:BUILD.semver-0.9.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__semver_parser__0_7_0",
        url = "https://crates.io/api/v1/crates/semver-parser/0.7.0/download",
        type = "tar.gz",
        sha256 = "388a1df253eca08550bef6c72392cfe7c30914bf41df5269b68cbd6ff8f570a3",
        strip_prefix = "semver-parser-0.7.0",
        build_file = Label("//third_party/remote:BUILD.semver-parser-0.7.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__serde__1_0_118",
        url = "https://crates.io/api/v1/crates/serde/1.0.118/download",
        type = "tar.gz",
        sha256 = "06c64263859d87aa2eb554587e2d23183398d617427327cf2b3d0ed8c69e4800",
        strip_prefix = "serde-1.0.118",
        build_file = Label("//third_party/remote:BUILD.serde-1.0.118.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__serde_derive__1_0_118",
        url = "https://crates.io/api/v1/crates/serde_derive/1.0.118/download",
        type = "tar.gz",
        sha256 = "c84d3526699cd55261af4b941e4e725444df67aa4f9e6a3564f18030d12672df",
        strip_prefix = "serde_derive-1.0.118",
        build_file = Label("//third_party/remote:BUILD.serde_derive-1.0.118.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__serde_json__1_0_61",
        url = "https://crates.io/api/v1/crates/serde_json/1.0.61/download",
        type = "tar.gz",
        sha256 = "4fceb2595057b6891a4ee808f70054bd2d12f0e97f1cbb78689b59f676df325a",
        strip_prefix = "serde_json-1.0.61",
        build_file = Label("//third_party/remote:BUILD.serde_json-1.0.61.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__sha_1__0_8_2",
        url = "https://crates.io/api/v1/crates/sha-1/0.8.2/download",
        type = "tar.gz",
        sha256 = "f7d94d0bede923b3cea61f3f1ff57ff8cdfd77b400fb8f9998949e0cf04163df",
        strip_prefix = "sha-1-0.8.2",
        build_file = Label("//third_party/remote:BUILD.sha-1-0.8.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__slab__0_4_2",
        url = "https://crates.io/api/v1/crates/slab/0.4.2/download",
        type = "tar.gz",
        sha256 = "c111b5bd5695e56cffe5129854aa230b39c93a305372fdbb2668ca2394eea9f8",
        strip_prefix = "slab-0.4.2",
        build_file = Label("//third_party/remote:BUILD.slab-0.4.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__smallvec__0_6_13",
        url = "https://crates.io/api/v1/crates/smallvec/0.6.13/download",
        type = "tar.gz",
        sha256 = "f7b0758c52e15a8b5e3691eae6cc559f08eee9406e548a4477ba4e67770a82b6",
        strip_prefix = "smallvec-0.6.13",
        build_file = Label("//third_party/remote:BUILD.smallvec-0.6.13.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__smallvec__1_6_0",
        url = "https://crates.io/api/v1/crates/smallvec/1.6.0/download",
        type = "tar.gz",
        sha256 = "1a55ca5f3b68e41c979bf8c46a6f1da892ca4db8f94023ce0bd32407573b1ac0",
        strip_prefix = "smallvec-1.6.0",
        build_file = Label("//third_party/remote:BUILD.smallvec-1.6.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__socket2__0_3_19",
        url = "https://crates.io/api/v1/crates/socket2/0.3.19/download",
        type = "tar.gz",
        sha256 = "122e570113d28d773067fab24266b66753f6ea915758651696b6e35e49f88d6e",
        strip_prefix = "socket2-0.3.19",
        build_file = Label("//third_party/remote:BUILD.socket2-0.3.19.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__stable_deref_trait__1_2_0",
        url = "https://crates.io/api/v1/crates/stable_deref_trait/1.2.0/download",
        type = "tar.gz",
        sha256 = "a8f112729512f8e442d81f95a8a7ddf2b7c6b8a1a6f509a95864142b30cab2d3",
        strip_prefix = "stable_deref_trait-1.2.0",
        build_file = Label("//third_party/remote:BUILD.stable_deref_trait-1.2.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__stacker__0_1_12",
        url = "https://crates.io/api/v1/crates/stacker/0.1.12/download",
        type = "tar.gz",
        sha256 = "21ccb4c06ec57bc82d0f610f1a2963d7648700e43a6f513e564b9c89f7991786",
        strip_prefix = "stacker-0.1.12",
        build_file = Label("//third_party/remote:BUILD.stacker-0.1.12.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__stdext__0_2_1",
        url = "https://crates.io/api/v1/crates/stdext/0.2.1/download",
        type = "tar.gz",
        sha256 = "4a61b4ae487cb43d0479907e74d36f8813e9940bd3b1adcbecc69fe8a0cee3ec",
        strip_prefix = "stdext-0.2.1",
        build_file = Label("//third_party/remote:BUILD.stdext-0.2.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__strsim__0_8_0",
        url = "https://crates.io/api/v1/crates/strsim/0.8.0/download",
        type = "tar.gz",
        sha256 = "8ea5119cdb4c55b55d432abb513a0429384878c15dde60cc77b1c99de1a95a6a",
        strip_prefix = "strsim-0.8.0",
        build_file = Label("//third_party/remote:BUILD.strsim-0.8.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__structopt__0_3_21",
        url = "https://crates.io/api/v1/crates/structopt/0.3.21/download",
        type = "tar.gz",
        sha256 = "5277acd7ee46e63e5168a80734c9f6ee81b1367a7d8772a2d765df2a3705d28c",
        strip_prefix = "structopt-0.3.21",
        build_file = Label("//third_party/remote:BUILD.structopt-0.3.21.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__structopt_derive__0_4_14",
        url = "https://crates.io/api/v1/crates/structopt-derive/0.4.14/download",
        type = "tar.gz",
        sha256 = "5ba9cdfda491b814720b6b06e0cac513d922fc407582032e8706e9f137976f90",
        strip_prefix = "structopt-derive-0.4.14",
        build_file = Label("//third_party/remote:BUILD.structopt-derive-0.4.14.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__syn__1_0_57",
        url = "https://crates.io/api/v1/crates/syn/1.0.57/download",
        type = "tar.gz",
        sha256 = "4211ce9909eb971f111059df92c45640aad50a619cf55cd76476be803c4c68e6",
        strip_prefix = "syn-1.0.57",
        build_file = Label("//third_party/remote:BUILD.syn-1.0.57.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__synstructure__0_12_4",
        url = "https://crates.io/api/v1/crates/synstructure/0.12.4/download",
        type = "tar.gz",
        sha256 = "b834f2d66f734cb897113e34aaff2f1ab4719ca946f9a7358dba8f8064148701",
        strip_prefix = "synstructure-0.12.4",
        build_file = Label("//third_party/remote:BUILD.synstructure-0.12.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tempfile__3_1_0",
        url = "https://crates.io/api/v1/crates/tempfile/3.1.0/download",
        type = "tar.gz",
        sha256 = "7a6e24d9338a0a5be79593e2fa15a648add6138caa803e2d5bc782c371732ca9",
        strip_prefix = "tempfile-3.1.0",
        build_file = Label("//third_party/remote:BUILD.tempfile-3.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__term__0_6_1",
        url = "https://crates.io/api/v1/crates/term/0.6.1/download",
        type = "tar.gz",
        sha256 = "c0863a3345e70f61d613eab32ee046ccd1bcc5f9105fe402c61fcd0c13eeb8b5",
        strip_prefix = "term-0.6.1",
        build_file = Label("//third_party/remote:BUILD.term-0.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__termcolor__1_1_2",
        url = "https://crates.io/api/v1/crates/termcolor/1.1.2/download",
        type = "tar.gz",
        sha256 = "2dfed899f0eb03f32ee8c6a0aabdb8a7949659e3466561fc0adf54e26d88c5f4",
        strip_prefix = "termcolor-1.1.2",
        build_file = Label("//third_party/remote:BUILD.termcolor-1.1.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__termize__0_1_1",
        url = "https://crates.io/api/v1/crates/termize/0.1.1/download",
        type = "tar.gz",
        sha256 = "1706be6b564323ce7092f5f7e6b118a14c8ef7ed0e69c8c5329c914a9f101295",
        strip_prefix = "termize-0.1.1",
        build_file = Label("//third_party/remote:BUILD.termize-0.1.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__textwrap__0_11_0",
        url = "https://crates.io/api/v1/crates/textwrap/0.11.0/download",
        type = "tar.gz",
        sha256 = "d326610f408c7a4eb6f51c37c330e496b08506c9457c9d34287ecc38809fb060",
        strip_prefix = "textwrap-0.11.0",
        build_file = Label("//third_party/remote:BUILD.textwrap-0.11.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__thiserror__1_0_23",
        url = "https://crates.io/api/v1/crates/thiserror/1.0.23/download",
        type = "tar.gz",
        sha256 = "76cc616c6abf8c8928e2fdcc0dbfab37175edd8fb49a4641066ad1364fdab146",
        strip_prefix = "thiserror-1.0.23",
        build_file = Label("//third_party/remote:BUILD.thiserror-1.0.23.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__thiserror_impl__1_0_23",
        url = "https://crates.io/api/v1/crates/thiserror-impl/1.0.23/download",
        type = "tar.gz",
        sha256 = "9be73a2caec27583d0046ef3796c3794f868a5bc813db689eed00c7631275cd1",
        strip_prefix = "thiserror-impl-1.0.23",
        build_file = Label("//third_party/remote:BUILD.thiserror-impl-1.0.23.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__thread_local__1_0_1",
        url = "https://crates.io/api/v1/crates/thread_local/1.0.1/download",
        type = "tar.gz",
        sha256 = "d40c6d1b69745a6ec6fb1ca717914848da4b44ae29d9b3080cbee91d72a69b14",
        strip_prefix = "thread_local-1.0.1",
        build_file = Label("//third_party/remote:BUILD.thread_local-1.0.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tinyvec__1_1_0",
        url = "https://crates.io/api/v1/crates/tinyvec/1.1.0/download",
        type = "tar.gz",
        sha256 = "ccf8dbc19eb42fba10e8feaaec282fb50e2c14b2726d6301dbfeed0f73306a6f",
        strip_prefix = "tinyvec-1.1.0",
        build_file = Label("//third_party/remote:BUILD.tinyvec-1.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tinyvec_macros__0_1_0",
        url = "https://crates.io/api/v1/crates/tinyvec_macros/0.1.0/download",
        type = "tar.gz",
        sha256 = "cda74da7e1a664f795bb1f8a87ec406fb89a02522cf6e50620d016add6dbbf5c",
        strip_prefix = "tinyvec_macros-0.1.0",
        build_file = Label("//third_party/remote:BUILD.tinyvec_macros-0.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio__0_2_24",
        url = "https://crates.io/api/v1/crates/tokio/0.2.24/download",
        type = "tar.gz",
        sha256 = "099837d3464c16a808060bb3f02263b412f6fafcb5d01c533d309985fbeebe48",
        strip_prefix = "tokio-0.2.24",
        build_file = Label("//third_party/remote:BUILD.tokio-0.2.24.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio__1_0_1",
        url = "https://crates.io/api/v1/crates/tokio/1.0.1/download",
        type = "tar.gz",
        sha256 = "d258221f566b6c803c7b4714abadc080172b272090cdc5e244a6d4dd13c3a6bd",
        strip_prefix = "tokio-1.0.1",
        build_file = Label("//third_party/remote:BUILD.tokio-1.0.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio_macros__0_2_6",
        url = "https://crates.io/api/v1/crates/tokio-macros/0.2.6/download",
        type = "tar.gz",
        sha256 = "e44da00bfc73a25f814cd8d7e57a68a5c31b74b3152a0a1d1f590c97ed06265a",
        strip_prefix = "tokio-macros-0.2.6",
        build_file = Label("//third_party/remote:BUILD.tokio-macros-0.2.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio_stream__0_1_0",
        url = "https://crates.io/api/v1/crates/tokio-stream/0.1.0/download",
        type = "tar.gz",
        sha256 = "3f3be913b74b13210c8fe04b17ab833f5a124f45b93d0f99f59fff621f64392a",
        strip_prefix = "tokio-stream-0.1.0",
        build_file = Label("//third_party/remote:BUILD.tokio-stream-0.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio_test__0_4_0",
        url = "https://crates.io/api/v1/crates/tokio-test/0.4.0/download",
        type = "tar.gz",
        sha256 = "7c7d205f6f59b03f9e824ac86eaba635a98395f287756ecc8a06464779c399bf",
        strip_prefix = "tokio-test-0.4.0",
        build_file = Label("//third_party/remote:BUILD.tokio-test-0.4.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio_util__0_3_1",
        url = "https://crates.io/api/v1/crates/tokio-util/0.3.1/download",
        type = "tar.gz",
        sha256 = "be8242891f2b6cbef26a2d7e8605133c2c554cd35b3e4948ea892d6d68436499",
        strip_prefix = "tokio-util-0.3.1",
        build_file = Label("//third_party/remote:BUILD.tokio-util-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__toml__0_5_8",
        url = "https://crates.io/api/v1/crates/toml/0.5.8/download",
        type = "tar.gz",
        sha256 = "a31142970826733df8241ef35dc040ef98c679ab14d7c3e54d827099b3acecaa",
        strip_prefix = "toml-0.5.8",
        build_file = Label("//third_party/remote:BUILD.toml-0.5.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tonic__0_3_1",
        url = "https://crates.io/api/v1/crates/tonic/0.3.1/download",
        type = "tar.gz",
        sha256 = "74a5d6e7439ecf910463667080de772a9c7ddf26bc9fb4f3252ac3862e43337d",
        strip_prefix = "tonic-0.3.1",
        build_file = Label("//third_party/remote:BUILD.tonic-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tonic_build__0_3_1",
        url = "https://crates.io/api/v1/crates/tonic-build/0.3.1/download",
        type = "tar.gz",
        sha256 = "19970cf58f3acc820962be74c4021b8bbc8e8a1c4e3a02095d0aa60cde5f3633",
        strip_prefix = "tonic-build-0.3.1",
        build_file = Label("//third_party/remote:BUILD.tonic-build-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower__0_3_1",
        url = "https://crates.io/api/v1/crates/tower/0.3.1/download",
        type = "tar.gz",
        sha256 = "fd3169017c090b7a28fce80abaad0ab4f5566423677c9331bb320af7e49cfe62",
        strip_prefix = "tower-0.3.1",
        build_file = Label("//third_party/remote:BUILD.tower-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_balance__0_3_0",
        url = "https://crates.io/api/v1/crates/tower-balance/0.3.0/download",
        type = "tar.gz",
        sha256 = "a792277613b7052448851efcf98a2c433e6f1d01460832dc60bef676bc275d4c",
        strip_prefix = "tower-balance-0.3.0",
        build_file = Label("//third_party/remote:BUILD.tower-balance-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_buffer__0_3_0",
        url = "https://crates.io/api/v1/crates/tower-buffer/0.3.0/download",
        type = "tar.gz",
        sha256 = "c4887dc2a65d464c8b9b66e0e4d51c2fd6cf5b3373afc72805b0a60bce00446a",
        strip_prefix = "tower-buffer-0.3.0",
        build_file = Label("//third_party/remote:BUILD.tower-buffer-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_discover__0_3_0",
        url = "https://crates.io/api/v1/crates/tower-discover/0.3.0/download",
        type = "tar.gz",
        sha256 = "0f6b5000c3c54d269cc695dff28136bb33d08cbf1df2c48129e143ab65bf3c2a",
        strip_prefix = "tower-discover-0.3.0",
        build_file = Label("//third_party/remote:BUILD.tower-discover-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_layer__0_3_0",
        url = "https://crates.io/api/v1/crates/tower-layer/0.3.0/download",
        type = "tar.gz",
        sha256 = "a35d656f2638b288b33495d1053ea74c40dc05ec0b92084dd71ca5566c4ed1dc",
        strip_prefix = "tower-layer-0.3.0",
        build_file = Label("//third_party/remote:BUILD.tower-layer-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_limit__0_3_1",
        url = "https://crates.io/api/v1/crates/tower-limit/0.3.1/download",
        type = "tar.gz",
        sha256 = "92c3040c5dbed68abffaa0d4517ac1a454cd741044f33ab0eefab6b8d1361404",
        strip_prefix = "tower-limit-0.3.1",
        build_file = Label("//third_party/remote:BUILD.tower-limit-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_load__0_3_0",
        url = "https://crates.io/api/v1/crates/tower-load/0.3.0/download",
        type = "tar.gz",
        sha256 = "8cc79fc3afd07492b7966d7efa7c6c50f8ed58d768a6075dd7ae6591c5d2017b",
        strip_prefix = "tower-load-0.3.0",
        build_file = Label("//third_party/remote:BUILD.tower-load-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_load_shed__0_3_0",
        url = "https://crates.io/api/v1/crates/tower-load-shed/0.3.0/download",
        type = "tar.gz",
        sha256 = "9f021e23900173dc315feb4b6922510dae3e79c689b74c089112066c11f0ae4e",
        strip_prefix = "tower-load-shed-0.3.0",
        build_file = Label("//third_party/remote:BUILD.tower-load-shed-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_make__0_3_0",
        url = "https://crates.io/api/v1/crates/tower-make/0.3.0/download",
        type = "tar.gz",
        sha256 = "ce50370d644a0364bf4877ffd4f76404156a248d104e2cc234cd391ea5cdc965",
        strip_prefix = "tower-make-0.3.0",
        build_file = Label("//third_party/remote:BUILD.tower-make-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_ready_cache__0_3_1",
        url = "https://crates.io/api/v1/crates/tower-ready-cache/0.3.1/download",
        type = "tar.gz",
        sha256 = "4eabb6620e5481267e2ec832c780b31cad0c15dcb14ed825df5076b26b591e1f",
        strip_prefix = "tower-ready-cache-0.3.1",
        build_file = Label("//third_party/remote:BUILD.tower-ready-cache-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_retry__0_3_0",
        url = "https://crates.io/api/v1/crates/tower-retry/0.3.0/download",
        type = "tar.gz",
        sha256 = "e6727956aaa2f8957d4d9232b308fe8e4e65d99db30f42b225646e86c9b6a952",
        strip_prefix = "tower-retry-0.3.0",
        build_file = Label("//third_party/remote:BUILD.tower-retry-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_service__0_3_0",
        url = "https://crates.io/api/v1/crates/tower-service/0.3.0/download",
        type = "tar.gz",
        sha256 = "e987b6bf443f4b5b3b6f38704195592cca41c5bb7aedd3c3693c7081f8289860",
        strip_prefix = "tower-service-0.3.0",
        build_file = Label("//third_party/remote:BUILD.tower-service-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_timeout__0_3_0",
        url = "https://crates.io/api/v1/crates/tower-timeout/0.3.0/download",
        type = "tar.gz",
        sha256 = "127b8924b357be938823eaaec0608c482d40add25609481027b96198b2e4b31e",
        strip_prefix = "tower-timeout-0.3.0",
        build_file = Label("//third_party/remote:BUILD.tower-timeout-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_util__0_3_1",
        url = "https://crates.io/api/v1/crates/tower-util/0.3.1/download",
        type = "tar.gz",
        sha256 = "d1093c19826d33807c72511e68f73b4a0469a3f22c2bd5f7d5212178b4b89674",
        strip_prefix = "tower-util-0.3.1",
        build_file = Label("//third_party/remote:BUILD.tower-util-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tracing__0_1_22",
        url = "https://crates.io/api/v1/crates/tracing/0.1.22/download",
        type = "tar.gz",
        sha256 = "9f47026cdc4080c07e49b37087de021820269d996f581aac150ef9e5583eefe3",
        strip_prefix = "tracing-0.1.22",
        build_file = Label("//third_party/remote:BUILD.tracing-0.1.22.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tracing_attributes__0_1_11",
        url = "https://crates.io/api/v1/crates/tracing-attributes/0.1.11/download",
        type = "tar.gz",
        sha256 = "80e0ccfc3378da0cce270c946b676a376943f5cd16aeba64568e7939806f4ada",
        strip_prefix = "tracing-attributes-0.1.11",
        build_file = Label("//third_party/remote:BUILD.tracing-attributes-0.1.11.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tracing_core__0_1_17",
        url = "https://crates.io/api/v1/crates/tracing-core/0.1.17/download",
        type = "tar.gz",
        sha256 = "f50de3927f93d202783f4513cda820ab47ef17f624b03c096e86ef00c67e6b5f",
        strip_prefix = "tracing-core-0.1.17",
        build_file = Label("//third_party/remote:BUILD.tracing-core-0.1.17.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tracing_futures__0_2_4",
        url = "https://crates.io/api/v1/crates/tracing-futures/0.2.4/download",
        type = "tar.gz",
        sha256 = "ab7bb6f14721aa00656086e9335d363c5c8747bae02ebe32ea2c7dece5689b4c",
        strip_prefix = "tracing-futures-0.2.4",
        build_file = Label("//third_party/remote:BUILD.tracing-futures-0.2.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__try_lock__0_2_3",
        url = "https://crates.io/api/v1/crates/try-lock/0.2.3/download",
        type = "tar.gz",
        sha256 = "59547bce71d9c38b83d9c0e92b6066c4253371f15005def0c30d9657f50c7642",
        strip_prefix = "try-lock-0.2.3",
        build_file = Label("//third_party/remote:BUILD.try-lock-0.2.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__typenum__1_12_0",
        url = "https://crates.io/api/v1/crates/typenum/1.12.0/download",
        type = "tar.gz",
        sha256 = "373c8a200f9e67a0c95e62a4f52fbf80c23b4381c05a17845531982fa99e6b33",
        strip_prefix = "typenum-1.12.0",
        build_file = Label("//third_party/remote:BUILD.typenum-1.12.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__unicode_normalization__0_1_16",
        url = "https://crates.io/api/v1/crates/unicode-normalization/0.1.16/download",
        type = "tar.gz",
        sha256 = "a13e63ab62dbe32aeee58d1c5408d35c36c392bba5d9d3142287219721afe606",
        strip_prefix = "unicode-normalization-0.1.16",
        build_file = Label("//third_party/remote:BUILD.unicode-normalization-0.1.16.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__unicode_segmentation__1_7_1",
        url = "https://crates.io/api/v1/crates/unicode-segmentation/1.7.1/download",
        type = "tar.gz",
        sha256 = "bb0d2e7be6ae3a5fa87eed5fb451aff96f2573d2694942e40543ae0bbe19c796",
        strip_prefix = "unicode-segmentation-1.7.1",
        build_file = Label("//third_party/remote:BUILD.unicode-segmentation-1.7.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__unicode_width__0_1_8",
        url = "https://crates.io/api/v1/crates/unicode-width/0.1.8/download",
        type = "tar.gz",
        sha256 = "9337591893a19b88d8d87f2cec1e73fad5cdfd10e5a6f349f498ad6ea2ffb1e3",
        strip_prefix = "unicode-width-0.1.8",
        build_file = Label("//third_party/remote:BUILD.unicode-width-0.1.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__unicode_xid__0_2_1",
        url = "https://crates.io/api/v1/crates/unicode-xid/0.2.1/download",
        type = "tar.gz",
        sha256 = "f7fe0bb3479651439c9112f72b6c505038574c9fbb575ed1bf3b797fa39dd564",
        strip_prefix = "unicode-xid-0.2.1",
        build_file = Label("//third_party/remote:BUILD.unicode-xid-0.2.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__unicode_categories__0_1_1",
        url = "https://crates.io/api/v1/crates/unicode_categories/0.1.1/download",
        type = "tar.gz",
        sha256 = "39ec24b3121d976906ece63c9daad25b85969647682eee313cb5779fdd69e14e",
        strip_prefix = "unicode_categories-0.1.1",
        build_file = Label("//third_party/remote:BUILD.unicode_categories-0.1.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__vec_map__0_8_2",
        url = "https://crates.io/api/v1/crates/vec_map/0.8.2/download",
        type = "tar.gz",
        sha256 = "f1bddf1187be692e79c5ffeab891132dfb0f236ed36a43c7ed39f1165ee20191",
        strip_prefix = "vec_map-0.8.2",
        build_file = Label("//third_party/remote:BUILD.vec_map-0.8.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__version_check__0_9_2",
        url = "https://crates.io/api/v1/crates/version_check/0.9.2/download",
        type = "tar.gz",
        sha256 = "b5a972e5669d67ba988ce3dc826706fb0a8b01471c088cb0b6110b805cc36aed",
        strip_prefix = "version_check-0.9.2",
        build_file = Label("//third_party/remote:BUILD.version_check-0.9.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__walkdir__2_3_1",
        url = "https://crates.io/api/v1/crates/walkdir/2.3.1/download",
        type = "tar.gz",
        sha256 = "777182bc735b6424e1a57516d35ed72cb8019d85c8c9bf536dccb3445c1a2f7d",
        strip_prefix = "walkdir-2.3.1",
        build_file = Label("//third_party/remote:BUILD.walkdir-2.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__want__0_3_0",
        url = "https://crates.io/api/v1/crates/want/0.3.0/download",
        type = "tar.gz",
        sha256 = "1ce8a968cb1cd110d136ff8b819a556d6fb6d919363c61534f6860c7eb172ba0",
        strip_prefix = "want-0.3.0",
        build_file = Label("//third_party/remote:BUILD.want-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__wasi__0_9_0_wasi_snapshot_preview1",
        url = "https://crates.io/api/v1/crates/wasi/0.9.0+wasi-snapshot-preview1/download",
        type = "tar.gz",
        sha256 = "cccddf32554fecc6acb585f82a32a72e28b48f8c4c1883ddfeeeaa96f7d8e519",
        strip_prefix = "wasi-0.9.0+wasi-snapshot-preview1",
        build_file = Label("//third_party/remote:BUILD.wasi-0.9.0+wasi-snapshot-preview1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__which__3_1_1",
        url = "https://crates.io/api/v1/crates/which/3.1.1/download",
        type = "tar.gz",
        sha256 = "d011071ae14a2f6671d0b74080ae0cd8ebf3a6f8c9589a2cd45f23126fe29724",
        strip_prefix = "which-3.1.1",
        build_file = Label("//third_party/remote:BUILD.which-3.1.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__winapi__0_2_8",
        url = "https://crates.io/api/v1/crates/winapi/0.2.8/download",
        type = "tar.gz",
        sha256 = "167dc9d6949a9b857f3451275e911c3f44255842c1f7a76f33c55103a909087a",
        strip_prefix = "winapi-0.2.8",
        build_file = Label("//third_party/remote:BUILD.winapi-0.2.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__winapi__0_3_9",
        url = "https://crates.io/api/v1/crates/winapi/0.3.9/download",
        type = "tar.gz",
        sha256 = "5c839a674fcd7a98952e593242ea400abe93992746761e38641405d28b00f419",
        strip_prefix = "winapi-0.3.9",
        build_file = Label("//third_party/remote:BUILD.winapi-0.3.9.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__winapi_build__0_1_1",
        url = "https://crates.io/api/v1/crates/winapi-build/0.1.1/download",
        type = "tar.gz",
        sha256 = "2d315eee3b34aca4797b2da6b13ed88266e6d612562a0c46390af8299fc699bc",
        strip_prefix = "winapi-build-0.1.1",
        build_file = Label("//third_party/remote:BUILD.winapi-build-0.1.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__winapi_i686_pc_windows_gnu__0_4_0",
        url = "https://crates.io/api/v1/crates/winapi-i686-pc-windows-gnu/0.4.0/download",
        type = "tar.gz",
        sha256 = "ac3b87c63620426dd9b991e5ce0329eff545bccbbb34f3be09ff6fb6ab51b7b6",
        strip_prefix = "winapi-i686-pc-windows-gnu-0.4.0",
        build_file = Label("//third_party/remote:BUILD.winapi-i686-pc-windows-gnu-0.4.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__winapi_util__0_1_5",
        url = "https://crates.io/api/v1/crates/winapi-util/0.1.5/download",
        type = "tar.gz",
        sha256 = "70ec6ce85bb158151cae5e5c87f95a8e97d2c0c4b001223f33a334e3ce5de178",
        strip_prefix = "winapi-util-0.1.5",
        build_file = Label("//third_party/remote:BUILD.winapi-util-0.1.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__winapi_x86_64_pc_windows_gnu__0_4_0",
        url = "https://crates.io/api/v1/crates/winapi-x86_64-pc-windows-gnu/0.4.0/download",
        type = "tar.gz",
        sha256 = "712e227841d057c1ee1cd2fb22fa7e5a5461ae8e48fa2ca79ec42cfc1931183f",
        strip_prefix = "winapi-x86_64-pc-windows-gnu-0.4.0",
        build_file = Label("//third_party/remote:BUILD.winapi-x86_64-pc-windows-gnu-0.4.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ws2_32_sys__0_2_1",
        url = "https://crates.io/api/v1/crates/ws2_32-sys/0.2.1/download",
        type = "tar.gz",
        sha256 = "d59cefebd0c892fa2dd6de581e937301d8552cb44489cdff035c6187cb63fa5e",
        strip_prefix = "ws2_32-sys-0.2.1",
        build_file = Label("//third_party/remote:BUILD.ws2_32-sys-0.2.1.bazel"),
    )
