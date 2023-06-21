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
        name = "raze__adler__1_0_2",
        url = "https://crates.io/api/v1/crates/adler/1.0.2/download",
        type = "tar.gz",
        sha256 = "f26201604c87b1e01bd3d98f8d5d9a8fcbb815e8cedb41ffccbeb4bf593a35fe",
        strip_prefix = "adler-1.0.2",
        build_file = Label("//third_party/remote:BUILD.adler-1.0.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ahash__0_8_3",
        url = "https://crates.io/api/v1/crates/ahash/0.8.3/download",
        type = "tar.gz",
        sha256 = "2c99f64d1e06488f620f932677e24bc6e2897582980441ae90a671415bd7ec2f",
        strip_prefix = "ahash-0.8.3",
        build_file = Label("//third_party/remote:BUILD.ahash-0.8.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__aho_corasick__1_0_2",
        url = "https://crates.io/api/v1/crates/aho-corasick/1.0.2/download",
        type = "tar.gz",
        sha256 = "43f6cb1bf222025340178f382c426f13757b2960e89779dfcb319c32542a5a41",
        strip_prefix = "aho-corasick-1.0.2",
        build_file = Label("//third_party/remote:BUILD.aho-corasick-1.0.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__android_tzdata__0_1_1",
        url = "https://crates.io/api/v1/crates/android-tzdata/0.1.1/download",
        type = "tar.gz",
        sha256 = "e999941b234f3131b00bc13c22d06e8c5ff726d1b6318ac7eb276997bbb4fef0",
        strip_prefix = "android-tzdata-0.1.1",
        build_file = Label("//third_party/remote:BUILD.android-tzdata-0.1.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__android_system_properties__0_1_5",
        url = "https://crates.io/api/v1/crates/android_system_properties/0.1.5/download",
        type = "tar.gz",
        sha256 = "819e7219dbd41043ac279b19830f2efc897156490d7fd6ea916720117ee66311",
        strip_prefix = "android_system_properties-0.1.5",
        build_file = Label("//third_party/remote:BUILD.android_system_properties-0.1.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ansi_term__0_12_1",
        url = "https://crates.io/api/v1/crates/ansi_term/0.12.1/download",
        type = "tar.gz",
        sha256 = "d52a9bb7ec0cf484c551830a7ce27bd20d67eac647e1befb56b0be4ee39a55d2",
        strip_prefix = "ansi_term-0.12.1",
        build_file = Label("//third_party/remote:BUILD.ansi_term-0.12.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__anstream__0_3_2",
        url = "https://crates.io/api/v1/crates/anstream/0.3.2/download",
        type = "tar.gz",
        sha256 = "0ca84f3628370c59db74ee214b3263d58f9aadd9b4fe7e711fd87dc452b7f163",
        strip_prefix = "anstream-0.3.2",
        build_file = Label("//third_party/remote:BUILD.anstream-0.3.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__anstyle__1_0_1",
        url = "https://crates.io/api/v1/crates/anstyle/1.0.1/download",
        type = "tar.gz",
        sha256 = "3a30da5c5f2d5e72842e00bcb57657162cdabef0931f40e2deb9b4140440cecd",
        strip_prefix = "anstyle-1.0.1",
        build_file = Label("//third_party/remote:BUILD.anstyle-1.0.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__anstyle_parse__0_2_1",
        url = "https://crates.io/api/v1/crates/anstyle-parse/0.2.1/download",
        type = "tar.gz",
        sha256 = "938874ff5980b03a87c5524b3ae5b59cf99b1d6bc836848df7bc5ada9643c333",
        strip_prefix = "anstyle-parse-0.2.1",
        build_file = Label("//third_party/remote:BUILD.anstyle-parse-0.2.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__anstyle_query__1_0_0",
        url = "https://crates.io/api/v1/crates/anstyle-query/1.0.0/download",
        type = "tar.gz",
        sha256 = "5ca11d4be1bab0c8bc8734a9aa7bf4ee8316d462a08c6ac5052f888fef5b494b",
        strip_prefix = "anstyle-query-1.0.0",
        build_file = Label("//third_party/remote:BUILD.anstyle-query-1.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__anstyle_wincon__1_0_1",
        url = "https://crates.io/api/v1/crates/anstyle-wincon/1.0.1/download",
        type = "tar.gz",
        sha256 = "180abfa45703aebe0093f79badacc01b8fd4ea2e35118747e5811127f926e188",
        strip_prefix = "anstyle-wincon-1.0.1",
        build_file = Label("//third_party/remote:BUILD.anstyle-wincon-1.0.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__anyhow__1_0_71",
        url = "https://crates.io/api/v1/crates/anyhow/1.0.71/download",
        type = "tar.gz",
        sha256 = "9c7d0618f0e0b7e8ff11427422b64564d5fb0be1940354bfe2e0529b18a9d9b8",
        strip_prefix = "anyhow-1.0.71",
        build_file = Label("//third_party/remote:BUILD.anyhow-1.0.71.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__async_stream__0_3_5",
        url = "https://crates.io/api/v1/crates/async-stream/0.3.5/download",
        type = "tar.gz",
        sha256 = "cd56dd203fef61ac097dd65721a419ddccb106b2d2b70ba60a6b529f03961a51",
        strip_prefix = "async-stream-0.3.5",
        build_file = Label("//third_party/remote:BUILD.async-stream-0.3.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__async_stream_impl__0_3_5",
        url = "https://crates.io/api/v1/crates/async-stream-impl/0.3.5/download",
        type = "tar.gz",
        sha256 = "16e62a023e7c117e27523144c5d2459f4397fcc3cab0085af8e2224f643a0193",
        strip_prefix = "async-stream-impl-0.3.5",
        build_file = Label("//third_party/remote:BUILD.async-stream-impl-0.3.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__async_trait__0_1_68",
        url = "https://crates.io/api/v1/crates/async-trait/0.1.68/download",
        type = "tar.gz",
        sha256 = "b9ccdd8f2a161be9bd5c023df56f1b2a0bd1d83872ae53b71a84a12c9bf6e842",
        strip_prefix = "async-trait-0.1.68",
        build_file = Label("//third_party/remote:BUILD.async-trait-0.1.68.bazel"),
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
        name = "raze__autocfg__1_1_0",
        url = "https://crates.io/api/v1/crates/autocfg/1.1.0/download",
        type = "tar.gz",
        sha256 = "d468802bab17cbc0cc575e9b053f41e72aa36bfa6b7f55e3529ffa43161b97fa",
        strip_prefix = "autocfg-1.1.0",
        build_file = Label("//third_party/remote:BUILD.autocfg-1.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__axum__0_6_18",
        url = "https://crates.io/api/v1/crates/axum/0.6.18/download",
        type = "tar.gz",
        sha256 = "f8175979259124331c1d7bf6586ee7e0da434155e4b2d48ec2c8386281d8df39",
        strip_prefix = "axum-0.6.18",
        build_file = Label("//third_party/remote:BUILD.axum-0.6.18.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__axum_core__0_3_4",
        url = "https://crates.io/api/v1/crates/axum-core/0.3.4/download",
        type = "tar.gz",
        sha256 = "759fa577a247914fd3f7f76d62972792636412fbfd634cd452f6a385a74d2d2c",
        strip_prefix = "axum-core-0.3.4",
        build_file = Label("//third_party/remote:BUILD.axum-core-0.3.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__base64__0_13_1",
        url = "https://crates.io/api/v1/crates/base64/0.13.1/download",
        type = "tar.gz",
        sha256 = "9e1b586273c5702936fe7b7d6896644d8be71e6314cfe09d3167c95f712589e8",
        strip_prefix = "base64-0.13.1",
        build_file = Label("//third_party/remote:BUILD.base64-0.13.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__bincode__1_3_3",
        url = "https://crates.io/api/v1/crates/bincode/1.3.3/download",
        type = "tar.gz",
        sha256 = "b1f45e9417d87227c7a56d22e471c6206462cba514c7590c09aff4cf6d1ddcad",
        strip_prefix = "bincode-1.3.3",
        build_file = Label("//third_party/remote:BUILD.bincode-1.3.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__bitflags__1_3_2",
        url = "https://crates.io/api/v1/crates/bitflags/1.3.2/download",
        type = "tar.gz",
        sha256 = "bef38d45163c2f1dde094a7dfd33ccf595c92905c8f8f4fdc18d06fb1037718a",
        strip_prefix = "bitflags-1.3.2",
        build_file = Label("//third_party/remote:BUILD.bitflags-1.3.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__block_buffer__0_10_4",
        url = "https://crates.io/api/v1/crates/block-buffer/0.10.4/download",
        type = "tar.gz",
        sha256 = "3078c7629b62d3f0439517fa394996acacc5cbc91c5a20d8c658e77abd503a71",
        strip_prefix = "block-buffer-0.10.4",
        build_file = Label("//third_party/remote:BUILD.block-buffer-0.10.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__block_buffer__0_9_0",
        url = "https://crates.io/api/v1/crates/block-buffer/0.9.0/download",
        type = "tar.gz",
        sha256 = "4152116fd6e9dadb291ae18fc1ec3575ed6d84c29642d97890f4b4a3417297e4",
        strip_prefix = "block-buffer-0.9.0",
        build_file = Label("//third_party/remote:BUILD.block-buffer-0.9.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__bumpalo__3_13_0",
        url = "https://crates.io/api/v1/crates/bumpalo/3.13.0/download",
        type = "tar.gz",
        sha256 = "a3e2c3daef883ecc1b5d58c15adae93470a91d425f3532ba1695849656af3fc1",
        strip_prefix = "bumpalo-3.13.0",
        build_file = Label("//third_party/remote:BUILD.bumpalo-3.13.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__byteorder__1_4_3",
        url = "https://crates.io/api/v1/crates/byteorder/1.4.3/download",
        type = "tar.gz",
        sha256 = "14c189c53d098945499cdfa7ecc63567cf3886b3332b312a5b4585d8d3a6a610",
        strip_prefix = "byteorder-1.4.3",
        build_file = Label("//third_party/remote:BUILD.byteorder-1.4.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__bytes__1_4_0",
        url = "https://crates.io/api/v1/crates/bytes/1.4.0/download",
        type = "tar.gz",
        sha256 = "89b2fd2a0dcf38d7971e2194b6b6eebab45ae01067456a7fd93d5547a61b70be",
        strip_prefix = "bytes-1.4.0",
        build_file = Label("//third_party/remote:BUILD.bytes-1.4.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__cc__1_0_79",
        url = "https://crates.io/api/v1/crates/cc/1.0.79/download",
        type = "tar.gz",
        sha256 = "50d30906286121d95be3d479533b458f87493b30a4b5f79a607db8f5d11aa91f",
        strip_prefix = "cc-1.0.79",
        build_file = Label("//third_party/remote:BUILD.cc-1.0.79.bazel"),
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
        name = "raze__chrono__0_4_26",
        url = "https://crates.io/api/v1/crates/chrono/0.4.26/download",
        type = "tar.gz",
        sha256 = "ec837a71355b28f6556dbd569b37b3f363091c0bd4b2e735674521b4c5fd9bc5",
        strip_prefix = "chrono-0.4.26",
        build_file = Label("//third_party/remote:BUILD.chrono-0.4.26.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__clap__4_3_5",
        url = "https://crates.io/api/v1/crates/clap/4.3.5/download",
        type = "tar.gz",
        sha256 = "2686c4115cb0810d9a984776e197823d08ec94f176549a89a9efded477c456dc",
        strip_prefix = "clap-4.3.5",
        build_file = Label("//third_party/remote:BUILD.clap-4.3.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__clap_builder__4_3_5",
        url = "https://crates.io/api/v1/crates/clap_builder/4.3.5/download",
        type = "tar.gz",
        sha256 = "2e53afce1efce6ed1f633cf0e57612fe51db54a1ee4fd8f8503d078fe02d69ae",
        strip_prefix = "clap_builder-4.3.5",
        build_file = Label("//third_party/remote:BUILD.clap_builder-4.3.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__clap_derive__4_3_2",
        url = "https://crates.io/api/v1/crates/clap_derive/4.3.2/download",
        type = "tar.gz",
        sha256 = "b8cd2b2a819ad6eec39e8f1d6b53001af1e5469f8c177579cdaeb313115b825f",
        strip_prefix = "clap_derive-4.3.2",
        build_file = Label("//third_party/remote:BUILD.clap_derive-4.3.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__clap_lex__0_5_0",
        url = "https://crates.io/api/v1/crates/clap_lex/0.5.0/download",
        type = "tar.gz",
        sha256 = "2da6da31387c7e4ef160ffab6d5e7f00c42626fe39aea70a7b0f1773f7dd6c1b",
        strip_prefix = "clap_lex-0.5.0",
        build_file = Label("//third_party/remote:BUILD.clap_lex-0.5.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__colorchoice__1_0_0",
        url = "https://crates.io/api/v1/crates/colorchoice/1.0.0/download",
        type = "tar.gz",
        sha256 = "acbf1af155f9b9ef647e42cdc158db4b64a1b61f743629225fde6f3e0be2a7c7",
        strip_prefix = "colorchoice-1.0.0",
        build_file = Label("//third_party/remote:BUILD.colorchoice-1.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__core_foundation__0_9_3",
        url = "https://crates.io/api/v1/crates/core-foundation/0.9.3/download",
        type = "tar.gz",
        sha256 = "194a7a9e6de53fa55116934067c844d9d749312f75c6f6d0980e8c252f8c2146",
        strip_prefix = "core-foundation-0.9.3",
        build_file = Label("//third_party/remote:BUILD.core-foundation-0.9.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__core_foundation_sys__0_8_4",
        url = "https://crates.io/api/v1/crates/core-foundation-sys/0.8.4/download",
        type = "tar.gz",
        sha256 = "e496a50fda8aacccc86d7529e2c1e0892dbd0f898a6b5645b5561b89c3210efa",
        strip_prefix = "core-foundation-sys-0.8.4",
        build_file = Label("//third_party/remote:BUILD.core-foundation-sys-0.8.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__cpufeatures__0_2_8",
        url = "https://crates.io/api/v1/crates/cpufeatures/0.2.8/download",
        type = "tar.gz",
        sha256 = "03e69e28e9f7f77debdedbaafa2866e1de9ba56df55a8bd7cfc724c25a09987c",
        strip_prefix = "cpufeatures-0.2.8",
        build_file = Label("//third_party/remote:BUILD.cpufeatures-0.2.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__crc32fast__1_3_2",
        url = "https://crates.io/api/v1/crates/crc32fast/1.3.2/download",
        type = "tar.gz",
        sha256 = "b540bd8bc810d3885c6ea91e2018302f68baba2129ab3e88f32389ee9370880d",
        strip_prefix = "crc32fast-1.3.2",
        build_file = Label("//third_party/remote:BUILD.crc32fast-1.3.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__crypto_common__0_1_6",
        url = "https://crates.io/api/v1/crates/crypto-common/0.1.6/download",
        type = "tar.gz",
        sha256 = "1bfb12502f3fc46cca1bb51ac28df9d618d813cdc3d2f25b9fe775a34af26bb3",
        strip_prefix = "crypto-common-0.1.6",
        build_file = Label("//third_party/remote:BUILD.crypto-common-0.1.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__crypto_mac__0_11_1",
        url = "https://crates.io/api/v1/crates/crypto-mac/0.11.1/download",
        type = "tar.gz",
        sha256 = "b1d1a86f49236c215f271d40892d5fc950490551400b02ef360692c29815c714",
        strip_prefix = "crypto-mac-0.11.1",
        build_file = Label("//third_party/remote:BUILD.crypto-mac-0.11.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ctor__0_1_26",
        url = "https://crates.io/api/v1/crates/ctor/0.1.26/download",
        type = "tar.gz",
        sha256 = "6d2301688392eb071b0bf1a37be05c469d3cc4dbbd95df672fe28ab021e6a096",
        strip_prefix = "ctor-0.1.26",
        build_file = Label("//third_party/remote:BUILD.ctor-0.1.26.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__diff__0_1_13",
        url = "https://crates.io/api/v1/crates/diff/0.1.13/download",
        type = "tar.gz",
        sha256 = "56254986775e3233ffa9c4d7d3faaf6d36a2c09d30b20687e9f88bc8bafc16c8",
        strip_prefix = "diff-0.1.13",
        build_file = Label("//third_party/remote:BUILD.diff-0.1.13.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__digest__0_10_7",
        url = "https://crates.io/api/v1/crates/digest/0.10.7/download",
        type = "tar.gz",
        sha256 = "9ed9a281f7bc9b7576e61468ba615a66a5c8cfdff42420a70aa82701a3b1e292",
        strip_prefix = "digest-0.10.7",
        build_file = Label("//third_party/remote:BUILD.digest-0.10.7.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__digest__0_9_0",
        url = "https://crates.io/api/v1/crates/digest/0.9.0/download",
        type = "tar.gz",
        sha256 = "d3dd60d1080a57a05ab032377049e0591415d2b31afd7028356dbf3cc6dcb066",
        strip_prefix = "digest-0.9.0",
        build_file = Label("//third_party/remote:BUILD.digest-0.9.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__dirs__4_0_0",
        url = "https://crates.io/api/v1/crates/dirs/4.0.0/download",
        type = "tar.gz",
        sha256 = "ca3aa72a6f96ea37bbc5aa912f6788242832f75369bdfdadcb0e38423f100059",
        strip_prefix = "dirs-4.0.0",
        build_file = Label("//third_party/remote:BUILD.dirs-4.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__dirs_next__2_0_0",
        url = "https://crates.io/api/v1/crates/dirs-next/2.0.0/download",
        type = "tar.gz",
        sha256 = "b98cf8ebf19c3d1b223e151f99a4f9f0690dca41414773390fc824184ac833e1",
        strip_prefix = "dirs-next-2.0.0",
        build_file = Label("//third_party/remote:BUILD.dirs-next-2.0.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__dirs_sys__0_3_7",
        url = "https://crates.io/api/v1/crates/dirs-sys/0.3.7/download",
        type = "tar.gz",
        sha256 = "1b1d1d91c932ef41c0f2663aa8b0ca0342d444d842c06914aa0a7e352d0bada6",
        strip_prefix = "dirs-sys-0.3.7",
        build_file = Label("//third_party/remote:BUILD.dirs-sys-0.3.7.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__dirs_sys_next__0_1_2",
        url = "https://crates.io/api/v1/crates/dirs-sys-next/0.1.2/download",
        type = "tar.gz",
        sha256 = "4ebda144c4fe02d1f7ea1a7d9641b6fc6b580adcfa024ae48797ecdeb6825b4d",
        strip_prefix = "dirs-sys-next-0.1.2",
        build_file = Label("//third_party/remote:BUILD.dirs-sys-next-0.1.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__either__1_8_1",
        url = "https://crates.io/api/v1/crates/either/1.8.1/download",
        type = "tar.gz",
        sha256 = "7fcaabb2fef8c910e7f4c7ce9f67a1283a1715879a7c230ca9d6d1ae31f16d91",
        strip_prefix = "either-1.8.1",
        build_file = Label("//third_party/remote:BUILD.either-1.8.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__env_logger__0_9_3",
        url = "https://crates.io/api/v1/crates/env_logger/0.9.3/download",
        type = "tar.gz",
        sha256 = "a12e6657c4c97ebab115a42dcee77225f7f482cdd841cf7088c657a42e9e00e7",
        strip_prefix = "env_logger-0.9.3",
        build_file = Label("//third_party/remote:BUILD.env_logger-0.9.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__errno__0_3_1",
        url = "https://crates.io/api/v1/crates/errno/0.3.1/download",
        type = "tar.gz",
        sha256 = "4bcfec3a70f97c962c307b2d2c56e358cf1d00b558d74262b5f929ee8cc7e73a",
        strip_prefix = "errno-0.3.1",
        build_file = Label("//third_party/remote:BUILD.errno-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__errno_dragonfly__0_1_2",
        url = "https://crates.io/api/v1/crates/errno-dragonfly/0.1.2/download",
        type = "tar.gz",
        sha256 = "aa68f1b12764fab894d2755d2518754e71b4fd80ecfb822714a1206c2aab39bf",
        strip_prefix = "errno-dragonfly-0.1.2",
        build_file = Label("//third_party/remote:BUILD.errno-dragonfly-0.1.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__fast_async_mutex__0_6_7",
        url = "https://crates.io/api/v1/crates/fast-async-mutex/0.6.7/download",
        type = "tar.gz",
        sha256 = "b877ceff2e3d2922823bab7960826ce198181f9c25f75d67b913fde9237e2da6",
        strip_prefix = "fast-async-mutex-0.6.7",
        build_file = Label("//third_party/remote:BUILD.fast-async-mutex-0.6.7.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__fastrand__1_9_0",
        url = "https://crates.io/api/v1/crates/fastrand/1.9.0/download",
        type = "tar.gz",
        sha256 = "e51093e27b0797c359783294ca4f0a911c270184cb10f85783b118614a1501be",
        strip_prefix = "fastrand-1.9.0",
        build_file = Label("//third_party/remote:BUILD.fastrand-1.9.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__filetime__0_2_21",
        url = "https://crates.io/api/v1/crates/filetime/0.2.21/download",
        type = "tar.gz",
        sha256 = "5cbc844cecaee9d4443931972e1289c8ff485cb4cc2767cb03ca139ed6885153",
        strip_prefix = "filetime-0.2.21",
        build_file = Label("//third_party/remote:BUILD.filetime-0.2.21.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__fixed_buffer__0_2_3",
        url = "https://crates.io/api/v1/crates/fixed-buffer/0.2.3/download",
        type = "tar.gz",
        sha256 = "ee27d08e4f11444330d000b8620aec3eae623d5bedd3667f65a2ee871e93dfdb",
        strip_prefix = "fixed-buffer-0.2.3",
        build_file = Label("//third_party/remote:BUILD.fixed-buffer-0.2.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__fixedbitset__0_4_2",
        url = "https://crates.io/api/v1/crates/fixedbitset/0.4.2/download",
        type = "tar.gz",
        sha256 = "0ce7134b9999ecaf8bcd65542e436736ef32ddca1b3e06094cb6ec5755203b80",
        strip_prefix = "fixedbitset-0.4.2",
        build_file = Label("//third_party/remote:BUILD.fixedbitset-0.4.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__flate2__1_0_26",
        url = "https://crates.io/api/v1/crates/flate2/1.0.26/download",
        type = "tar.gz",
        sha256 = "3b9429470923de8e8cbd4d2dc513535400b4b3fef0319fb5c4e1f520a7bef743",
        strip_prefix = "flate2-1.0.26",
        build_file = Label("//third_party/remote:BUILD.flate2-1.0.26.bazel"),
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
        name = "raze__foreign_types__0_3_2",
        url = "https://crates.io/api/v1/crates/foreign-types/0.3.2/download",
        type = "tar.gz",
        sha256 = "f6f339eb8adc052cd2ca78910fda869aefa38d22d5cb648e6485e4d3fc06f3b1",
        strip_prefix = "foreign-types-0.3.2",
        build_file = Label("//third_party/remote:BUILD.foreign-types-0.3.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__foreign_types_shared__0_1_1",
        url = "https://crates.io/api/v1/crates/foreign-types-shared/0.1.1/download",
        type = "tar.gz",
        sha256 = "00b0228411908ca8685dba7fc2cdd70ec9990a6e753e89b6ac91a84c40fbaf4b",
        strip_prefix = "foreign-types-shared-0.1.1",
        build_file = Label("//third_party/remote:BUILD.foreign-types-shared-0.1.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures__0_3_28",
        url = "https://crates.io/api/v1/crates/futures/0.3.28/download",
        type = "tar.gz",
        sha256 = "23342abe12aba583913b2e62f22225ff9c950774065e4bfb61a19cd9770fec40",
        strip_prefix = "futures-0.3.28",
        build_file = Label("//third_party/remote:BUILD.futures-0.3.28.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_channel__0_3_28",
        url = "https://crates.io/api/v1/crates/futures-channel/0.3.28/download",
        type = "tar.gz",
        sha256 = "955518d47e09b25bbebc7a18df10b81f0c766eaf4c4f1cccef2fca5f2a4fb5f2",
        strip_prefix = "futures-channel-0.3.28",
        build_file = Label("//third_party/remote:BUILD.futures-channel-0.3.28.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_core__0_3_28",
        url = "https://crates.io/api/v1/crates/futures-core/0.3.28/download",
        type = "tar.gz",
        sha256 = "4bca583b7e26f571124fe5b7561d49cb2868d79116cfa0eefce955557c6fee8c",
        strip_prefix = "futures-core-0.3.28",
        build_file = Label("//third_party/remote:BUILD.futures-core-0.3.28.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_executor__0_3_28",
        url = "https://crates.io/api/v1/crates/futures-executor/0.3.28/download",
        type = "tar.gz",
        sha256 = "ccecee823288125bd88b4d7f565c9e58e41858e47ab72e8ea2d64e93624386e0",
        strip_prefix = "futures-executor-0.3.28",
        build_file = Label("//third_party/remote:BUILD.futures-executor-0.3.28.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_io__0_3_28",
        url = "https://crates.io/api/v1/crates/futures-io/0.3.28/download",
        type = "tar.gz",
        sha256 = "4fff74096e71ed47f8e023204cfd0aa1289cd54ae5430a9523be060cdb849964",
        strip_prefix = "futures-io-0.3.28",
        build_file = Label("//third_party/remote:BUILD.futures-io-0.3.28.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_macro__0_3_28",
        url = "https://crates.io/api/v1/crates/futures-macro/0.3.28/download",
        type = "tar.gz",
        sha256 = "89ca545a94061b6365f2c7355b4b32bd20df3ff95f02da9329b34ccc3bd6ee72",
        strip_prefix = "futures-macro-0.3.28",
        build_file = Label("//third_party/remote:BUILD.futures-macro-0.3.28.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_sink__0_3_28",
        url = "https://crates.io/api/v1/crates/futures-sink/0.3.28/download",
        type = "tar.gz",
        sha256 = "f43be4fe21a13b9781a69afa4985b0f6ee0e1afab2c6f454a8cf30e2b2237b6e",
        strip_prefix = "futures-sink-0.3.28",
        build_file = Label("//third_party/remote:BUILD.futures-sink-0.3.28.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_task__0_3_28",
        url = "https://crates.io/api/v1/crates/futures-task/0.3.28/download",
        type = "tar.gz",
        sha256 = "76d3d132be6c0e6aa1534069c705a74a5997a356c0dc2f86a47765e5617c5b65",
        strip_prefix = "futures-task-0.3.28",
        build_file = Label("//third_party/remote:BUILD.futures-task-0.3.28.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__futures_util__0_3_28",
        url = "https://crates.io/api/v1/crates/futures-util/0.3.28/download",
        type = "tar.gz",
        sha256 = "26b01e40b772d54cf6c6d721c1d1abd0647a0106a12ecaa1c186273392a69533",
        strip_prefix = "futures-util-0.3.28",
        build_file = Label("//third_party/remote:BUILD.futures-util-0.3.28.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__generic_array__0_14_7",
        url = "https://crates.io/api/v1/crates/generic-array/0.14.7/download",
        type = "tar.gz",
        sha256 = "85649ca51fd72272d7821adaf274ad91c288277713d9c18820d8499a7ff69e9a",
        strip_prefix = "generic-array-0.14.7",
        build_file = Label("//third_party/remote:BUILD.generic-array-0.14.7.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__getrandom__0_2_10",
        url = "https://crates.io/api/v1/crates/getrandom/0.2.10/download",
        type = "tar.gz",
        sha256 = "be4136b2a15dd319360be1c07d9933517ccf0be8f16bf62a3bee4f0d618df427",
        strip_prefix = "getrandom-0.2.10",
        build_file = Label("//third_party/remote:BUILD.getrandom-0.2.10.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__h2__0_3_19",
        url = "https://crates.io/api/v1/crates/h2/0.3.19/download",
        type = "tar.gz",
        sha256 = "d357c7ae988e7d2182f7d7871d0b963962420b0678b0997ce7de72001aeab782",
        strip_prefix = "h2-0.3.19",
        build_file = Label("//third_party/remote:BUILD.h2-0.3.19.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hashbrown__0_12_3",
        url = "https://crates.io/api/v1/crates/hashbrown/0.12.3/download",
        type = "tar.gz",
        sha256 = "8a9ee70c43aaf417c914396645a0fa852624801b24ebb7ae78fe8272889ac888",
        strip_prefix = "hashbrown-0.12.3",
        build_file = Label("//third_party/remote:BUILD.hashbrown-0.12.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hashbrown__0_13_2",
        url = "https://crates.io/api/v1/crates/hashbrown/0.13.2/download",
        type = "tar.gz",
        sha256 = "43a3c133739dddd0d2990f9a4bdf8eb4b21ef50e4851ca85ab661199821d510e",
        strip_prefix = "hashbrown-0.13.2",
        build_file = Label("//third_party/remote:BUILD.hashbrown-0.13.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__heck__0_4_1",
        url = "https://crates.io/api/v1/crates/heck/0.4.1/download",
        type = "tar.gz",
        sha256 = "95505c38b4572b2d910cecb0281560f54b440a19336cbbcb27bf6ce6adc6f5a8",
        strip_prefix = "heck-0.4.1",
        build_file = Label("//third_party/remote:BUILD.heck-0.4.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hermit_abi__0_1_19",
        url = "https://crates.io/api/v1/crates/hermit-abi/0.1.19/download",
        type = "tar.gz",
        sha256 = "62b467343b94ba476dcb2500d242dadbb39557df889310ac77c5d99100aaac33",
        strip_prefix = "hermit-abi-0.1.19",
        build_file = Label("//third_party/remote:BUILD.hermit-abi-0.1.19.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hermit_abi__0_2_6",
        url = "https://crates.io/api/v1/crates/hermit-abi/0.2.6/download",
        type = "tar.gz",
        sha256 = "ee512640fe35acbfb4bb779db6f0d80704c2cacfa2e39b601ef3e3f47d1ae4c7",
        strip_prefix = "hermit-abi-0.2.6",
        build_file = Label("//third_party/remote:BUILD.hermit-abi-0.2.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hermit_abi__0_3_1",
        url = "https://crates.io/api/v1/crates/hermit-abi/0.3.1/download",
        type = "tar.gz",
        sha256 = "fed44880c466736ef9a5c5b5facefb5ed0785676d0c02d612db14e54f0d84286",
        strip_prefix = "hermit-abi-0.3.1",
        build_file = Label("//third_party/remote:BUILD.hermit-abi-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hex__0_4_3",
        url = "https://crates.io/api/v1/crates/hex/0.4.3/download",
        type = "tar.gz",
        sha256 = "7f24254aa9a54b5c858eaee2f5bccdb46aaf0e486a595ed5fd8f86ba55232a70",
        strip_prefix = "hex-0.4.3",
        build_file = Label("//third_party/remote:BUILD.hex-0.4.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hmac__0_11_0",
        url = "https://crates.io/api/v1/crates/hmac/0.11.0/download",
        type = "tar.gz",
        sha256 = "2a2a2320eb7ec0ebe8da8f744d7812d9fc4cb4d09344ac01898dbcb6a20ae69b",
        strip_prefix = "hmac-0.11.0",
        build_file = Label("//third_party/remote:BUILD.hmac-0.11.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__http__0_2_9",
        url = "https://crates.io/api/v1/crates/http/0.2.9/download",
        type = "tar.gz",
        sha256 = "bd6effc99afb63425aff9b05836f029929e345a6148a14b7ecd5ab67af944482",
        strip_prefix = "http-0.2.9",
        build_file = Label("//third_party/remote:BUILD.http-0.2.9.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__http_body__0_4_5",
        url = "https://crates.io/api/v1/crates/http-body/0.4.5/download",
        type = "tar.gz",
        sha256 = "d5f38f16d184e36f2408a55281cd658ecbd3ca05cce6d6510a176eca393e26d1",
        strip_prefix = "http-body-0.4.5",
        build_file = Label("//third_party/remote:BUILD.http-body-0.4.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__httparse__1_8_0",
        url = "https://crates.io/api/v1/crates/httparse/1.8.0/download",
        type = "tar.gz",
        sha256 = "d897f394bad6a705d5f4104762e116a75639e470d80901eed05a860a95cb1904",
        strip_prefix = "httparse-1.8.0",
        build_file = Label("//third_party/remote:BUILD.httparse-1.8.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__httpdate__1_0_2",
        url = "https://crates.io/api/v1/crates/httpdate/1.0.2/download",
        type = "tar.gz",
        sha256 = "c4a1e36c821dbe04574f602848a19f742f4fb3c98d40449f11bcad18d6b17421",
        strip_prefix = "httpdate-1.0.2",
        build_file = Label("//third_party/remote:BUILD.httpdate-1.0.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__humantime__2_1_0",
        url = "https://crates.io/api/v1/crates/humantime/2.1.0/download",
        type = "tar.gz",
        sha256 = "9a3a5bfb195931eeb336b2a7b4d761daec841b97f947d34394601737a7bba5e4",
        strip_prefix = "humantime-2.1.0",
        build_file = Label("//third_party/remote:BUILD.humantime-2.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hyper__0_14_26",
        url = "https://crates.io/api/v1/crates/hyper/0.14.26/download",
        type = "tar.gz",
        sha256 = "ab302d72a6f11a3b910431ff93aae7e773078c769f0a3ef15fb9ec692ed147d4",
        strip_prefix = "hyper-0.14.26",
        build_file = Label("//third_party/remote:BUILD.hyper-0.14.26.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hyper_timeout__0_4_1",
        url = "https://crates.io/api/v1/crates/hyper-timeout/0.4.1/download",
        type = "tar.gz",
        sha256 = "bbb958482e8c7be4bc3cf272a766a2b0bf1a6755e7a6ae777f017a31d11b13b1",
        strip_prefix = "hyper-timeout-0.4.1",
        build_file = Label("//third_party/remote:BUILD.hyper-timeout-0.4.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__hyper_tls__0_5_0",
        url = "https://crates.io/api/v1/crates/hyper-tls/0.5.0/download",
        type = "tar.gz",
        sha256 = "d6183ddfa99b85da61a140bea0efc93fdf56ceaa041b37d553518030827f9905",
        strip_prefix = "hyper-tls-0.5.0",
        build_file = Label("//third_party/remote:BUILD.hyper-tls-0.5.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__iana_time_zone__0_1_57",
        url = "https://crates.io/api/v1/crates/iana-time-zone/0.1.57/download",
        type = "tar.gz",
        sha256 = "2fad5b825842d2b38bd206f3e81d6957625fd7f0a361e345c30e01a0ae2dd613",
        strip_prefix = "iana-time-zone-0.1.57",
        build_file = Label("//third_party/remote:BUILD.iana-time-zone-0.1.57.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__iana_time_zone_haiku__0_1_2",
        url = "https://crates.io/api/v1/crates/iana-time-zone-haiku/0.1.2/download",
        type = "tar.gz",
        sha256 = "f31827a206f56af32e590ba56d5d2d085f558508192593743f16b2306495269f",
        strip_prefix = "iana-time-zone-haiku-0.1.2",
        build_file = Label("//third_party/remote:BUILD.iana-time-zone-haiku-0.1.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__indexmap__1_9_3",
        url = "https://crates.io/api/v1/crates/indexmap/1.9.3/download",
        type = "tar.gz",
        sha256 = "bd070e393353796e801d209ad339e89596eb4c8d430d18ede6a1cced8fafbd99",
        strip_prefix = "indexmap-1.9.3",
        build_file = Label("//third_party/remote:BUILD.indexmap-1.9.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__instant__0_1_12",
        url = "https://crates.io/api/v1/crates/instant/0.1.12/download",
        type = "tar.gz",
        sha256 = "7a5bbe824c507c5da5956355e86a746d82e0e1464f65d862cc5e71da70e94b2c",
        strip_prefix = "instant-0.1.12",
        build_file = Label("//third_party/remote:BUILD.instant-0.1.12.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__io_lifetimes__1_0_11",
        url = "https://crates.io/api/v1/crates/io-lifetimes/1.0.11/download",
        type = "tar.gz",
        sha256 = "eae7b9aee968036d54dce06cebaefd919e4472e753296daccd6d344e3e2df0c2",
        strip_prefix = "io-lifetimes-1.0.11",
        build_file = Label("//third_party/remote:BUILD.io-lifetimes-1.0.11.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__is_terminal__0_4_7",
        url = "https://crates.io/api/v1/crates/is-terminal/0.4.7/download",
        type = "tar.gz",
        sha256 = "adcf93614601c8129ddf72e2d5633df827ba6551541c6d8c59520a371475be1f",
        strip_prefix = "is-terminal-0.4.7",
        build_file = Label("//third_party/remote:BUILD.is-terminal-0.4.7.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__itertools__0_10_5",
        url = "https://crates.io/api/v1/crates/itertools/0.10.5/download",
        type = "tar.gz",
        sha256 = "b0fd2260e829bddf4cb6ea802289de2f86d6a7a690192fbe91b3f46e0f2c8473",
        strip_prefix = "itertools-0.10.5",
        build_file = Label("//third_party/remote:BUILD.itertools-0.10.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__itoa__1_0_6",
        url = "https://crates.io/api/v1/crates/itoa/1.0.6/download",
        type = "tar.gz",
        sha256 = "453ad9f582a441959e5f0d088b02ce04cfe8d51a8eaf077f12ac6d3e94164ca6",
        strip_prefix = "itoa-1.0.6",
        build_file = Label("//third_party/remote:BUILD.itoa-1.0.6.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__js_sys__0_3_64",
        url = "https://crates.io/api/v1/crates/js-sys/0.3.64/download",
        type = "tar.gz",
        sha256 = "c5f195fe497f702db0f318b07fdd68edb16955aed830df8363d837542f8f935a",
        strip_prefix = "js-sys-0.3.64",
        build_file = Label("//third_party/remote:BUILD.js-sys-0.3.64.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__json5__0_3_0",
        url = "https://crates.io/api/v1/crates/json5/0.3.0/download",
        type = "tar.gz",
        sha256 = "3d993b17585f39e5e3bd98ff52bbd9e2a6d6b3f5b09d8abcec9d1873fb04cf3f",
        strip_prefix = "json5-0.3.0",
        build_file = Label("//third_party/remote:BUILD.json5-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__lazy_init__0_5_1",
        url = "https://crates.io/api/v1/crates/lazy-init/0.5.1/download",
        type = "tar.gz",
        sha256 = "9f40963626ac12dcaf92afc15e4c3db624858c92fd9f8ba2125eaada3ac2706f",
        strip_prefix = "lazy-init-0.5.1",
        build_file = Label("//third_party/remote:BUILD.lazy-init-0.5.1.bazel"),
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
        name = "raze__libc__0_2_146",
        url = "https://crates.io/api/v1/crates/libc/0.2.146/download",
        type = "tar.gz",
        sha256 = "f92be4933c13fd498862a9e02a3055f8a8d9c039ce33db97306fd5a6caa7f29b",
        strip_prefix = "libc-0.2.146",
        build_file = Label("//third_party/remote:BUILD.libc-0.2.146.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__linux_raw_sys__0_3_8",
        url = "https://crates.io/api/v1/crates/linux-raw-sys/0.3.8/download",
        type = "tar.gz",
        sha256 = "ef53942eb7bf7ff43a617b3e2c1c4a5ecf5944a7c1bc12d7ee39bbb15e5c1519",
        strip_prefix = "linux-raw-sys-0.3.8",
        build_file = Label("//third_party/remote:BUILD.linux-raw-sys-0.3.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__lock_api__0_4_10",
        url = "https://crates.io/api/v1/crates/lock_api/0.4.10/download",
        type = "tar.gz",
        sha256 = "c1cc9717a20b1bb222f333e6a92fd32f7d8a18ddc5a3191a11af45dcbf4dcd16",
        strip_prefix = "lock_api-0.4.10",
        build_file = Label("//third_party/remote:BUILD.lock_api-0.4.10.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__log__0_4_19",
        url = "https://crates.io/api/v1/crates/log/0.4.19/download",
        type = "tar.gz",
        sha256 = "b06a4cde4c0f271a446782e3eff8de789548ce57dbc8eca9292c27f4a42004b4",
        strip_prefix = "log-0.4.19",
        build_file = Label("//third_party/remote:BUILD.log-0.4.19.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__lru__0_9_0",
        url = "https://crates.io/api/v1/crates/lru/0.9.0/download",
        type = "tar.gz",
        sha256 = "71e7d46de488603ffdd5f30afbc64fbba2378214a2c3a2fb83abf3d33126df17",
        strip_prefix = "lru-0.9.0",
        build_file = Label("//third_party/remote:BUILD.lru-0.9.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__lz4_flex__0_9_5",
        url = "https://crates.io/api/v1/crates/lz4_flex/0.9.5/download",
        type = "tar.gz",
        sha256 = "1a8cbbb2831780bc3b9c15a41f5b49222ef756b6730a95f3decfdd15903eb5a3",
        strip_prefix = "lz4_flex-0.9.5",
        build_file = Label("//third_party/remote:BUILD.lz4_flex-0.9.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__maplit__1_0_2",
        url = "https://crates.io/api/v1/crates/maplit/1.0.2/download",
        type = "tar.gz",
        sha256 = "3e2e65a1a2e43cfcb47a895c4c8b10d1f4a61097f9f254f183aee60cad9c651d",
        strip_prefix = "maplit-1.0.2",
        build_file = Label("//third_party/remote:BUILD.maplit-1.0.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__matchit__0_7_0",
        url = "https://crates.io/api/v1/crates/matchit/0.7.0/download",
        type = "tar.gz",
        sha256 = "b87248edafb776e59e6ee64a79086f65890d3510f2c656c000bf2a7e8a0aea40",
        strip_prefix = "matchit-0.7.0",
        build_file = Label("//third_party/remote:BUILD.matchit-0.7.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__md_5__0_9_1",
        url = "https://crates.io/api/v1/crates/md-5/0.9.1/download",
        type = "tar.gz",
        sha256 = "7b5a279bb9607f9f53c22d496eade00d138d1bdcccd07d74650387cf94942a15",
        strip_prefix = "md-5-0.9.1",
        build_file = Label("//third_party/remote:BUILD.md-5-0.9.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__memchr__2_5_0",
        url = "https://crates.io/api/v1/crates/memchr/2.5.0/download",
        type = "tar.gz",
        sha256 = "2dffe52ecf27772e601905b7522cb4ef790d2cc203488bbd0e2fe85fcb74566d",
        strip_prefix = "memchr-2.5.0",
        build_file = Label("//third_party/remote:BUILD.memchr-2.5.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__memoffset__0_6_5",
        url = "https://crates.io/api/v1/crates/memoffset/0.6.5/download",
        type = "tar.gz",
        sha256 = "5aa361d4faea93603064a027415f07bd8e1d5c88c9fbf68bf56a285428fd79ce",
        strip_prefix = "memoffset-0.6.5",
        build_file = Label("//third_party/remote:BUILD.memoffset-0.6.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__mime__0_3_17",
        url = "https://crates.io/api/v1/crates/mime/0.3.17/download",
        type = "tar.gz",
        sha256 = "6877bb514081ee2a7ff5ef9de3281f14a4dd4bceac4c09388074a6b5df8a139a",
        strip_prefix = "mime-0.3.17",
        build_file = Label("//third_party/remote:BUILD.mime-0.3.17.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__miniz_oxide__0_7_1",
        url = "https://crates.io/api/v1/crates/miniz_oxide/0.7.1/download",
        type = "tar.gz",
        sha256 = "e7810e0be55b428ada41041c41f32c9f1a42817901b4ccf45fa3d4b6561e74c7",
        strip_prefix = "miniz_oxide-0.7.1",
        build_file = Label("//third_party/remote:BUILD.miniz_oxide-0.7.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__mio__0_8_8",
        url = "https://crates.io/api/v1/crates/mio/0.8.8/download",
        type = "tar.gz",
        sha256 = "927a765cd3fc26206e66b296465fa9d3e5ab003e651c1b3c060e7956d96b19d2",
        strip_prefix = "mio-0.8.8",
        build_file = Label("//third_party/remote:BUILD.mio-0.8.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__mock_instant__0_2_1",
        url = "https://crates.io/api/v1/crates/mock_instant/0.2.1/download",
        type = "tar.gz",
        sha256 = "717e29a243b81f8130e31e24e04fb151b04a44b5a7d05370935f7d937e9de06d",
        strip_prefix = "mock_instant-0.2.1",
        build_file = Label("//third_party/remote:BUILD.mock_instant-0.2.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__multimap__0_8_3",
        url = "https://crates.io/api/v1/crates/multimap/0.8.3/download",
        type = "tar.gz",
        sha256 = "e5ce46fe64a9d73be07dcbe690a38ce1b293be448fd8ce1e6c1b8062c9f72c6a",
        strip_prefix = "multimap-0.8.3",
        build_file = Label("//third_party/remote:BUILD.multimap-0.8.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__native_tls__0_2_11",
        url = "https://crates.io/api/v1/crates/native-tls/0.2.11/download",
        type = "tar.gz",
        sha256 = "07226173c32f2926027b63cce4bcd8076c3552846cbe7925f3aaffeac0a3b92e",
        strip_prefix = "native-tls-0.2.11",
        build_file = Label("//third_party/remote:BUILD.native-tls-0.2.11.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__nix__0_23_2",
        url = "https://crates.io/api/v1/crates/nix/0.23.2/download",
        type = "tar.gz",
        sha256 = "8f3790c00a0150112de0f4cd161e3d7fc4b2d8a5542ffc35f099a2562aecb35c",
        strip_prefix = "nix-0.23.2",
        build_file = Label("//third_party/remote:BUILD.nix-0.23.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__num_traits__0_2_15",
        url = "https://crates.io/api/v1/crates/num-traits/0.2.15/download",
        type = "tar.gz",
        sha256 = "578ede34cf02f8924ab9447f50c28075b4d3e5b269972345e7e0372b38c6cdcd",
        strip_prefix = "num-traits-0.2.15",
        build_file = Label("//third_party/remote:BUILD.num-traits-0.2.15.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__num_cpus__1_15_0",
        url = "https://crates.io/api/v1/crates/num_cpus/1.15.0/download",
        type = "tar.gz",
        sha256 = "0fac9e2da13b5eb447a6ce3d392f23a29d8694bff781bf03a16cd9ac8697593b",
        strip_prefix = "num_cpus-1.15.0",
        build_file = Label("//third_party/remote:BUILD.num_cpus-1.15.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__once_cell__1_18_0",
        url = "https://crates.io/api/v1/crates/once_cell/1.18.0/download",
        type = "tar.gz",
        sha256 = "dd8b5dd2ae5ed71462c540258bedcb51965123ad7e7ccf4b9a8cafaa4a63576d",
        strip_prefix = "once_cell-1.18.0",
        build_file = Label("//third_party/remote:BUILD.once_cell-1.18.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__opaque_debug__0_3_0",
        url = "https://crates.io/api/v1/crates/opaque-debug/0.3.0/download",
        type = "tar.gz",
        sha256 = "624a8340c38c1b80fd549087862da4ba43e08858af025b236e509b6649fc13d5",
        strip_prefix = "opaque-debug-0.3.0",
        build_file = Label("//third_party/remote:BUILD.opaque-debug-0.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__openssl__0_10_55",
        url = "https://crates.io/api/v1/crates/openssl/0.10.55/download",
        type = "tar.gz",
        sha256 = "345df152bc43501c5eb9e4654ff05f794effb78d4efe3d53abc158baddc0703d",
        strip_prefix = "openssl-0.10.55",
        build_file = Label("//third_party/remote:BUILD.openssl-0.10.55.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__openssl_macros__0_1_1",
        url = "https://crates.io/api/v1/crates/openssl-macros/0.1.1/download",
        type = "tar.gz",
        sha256 = "a948666b637a0f465e8564c73e89d4dde00d72d4d473cc972f390fc3dcee7d9c",
        strip_prefix = "openssl-macros-0.1.1",
        build_file = Label("//third_party/remote:BUILD.openssl-macros-0.1.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__openssl_probe__0_1_5",
        url = "https://crates.io/api/v1/crates/openssl-probe/0.1.5/download",
        type = "tar.gz",
        sha256 = "ff011a302c396a5197692431fc1948019154afc178baf7d8e37367442a4601cf",
        strip_prefix = "openssl-probe-0.1.5",
        build_file = Label("//third_party/remote:BUILD.openssl-probe-0.1.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__openssl_sys__0_9_90",
        url = "https://crates.io/api/v1/crates/openssl-sys/0.9.90/download",
        type = "tar.gz",
        sha256 = "374533b0e45f3a7ced10fcaeccca020e66656bc03dac384f852e4e5a7a8104a6",
        strip_prefix = "openssl-sys-0.9.90",
        build_file = Label("//third_party/remote:BUILD.openssl-sys-0.9.90.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__output_vt100__0_1_3",
        url = "https://crates.io/api/v1/crates/output_vt100/0.1.3/download",
        type = "tar.gz",
        sha256 = "628223faebab4e3e40667ee0b2336d34a5b960ff60ea743ddfdbcf7770bcfb66",
        strip_prefix = "output_vt100-0.1.3",
        build_file = Label("//third_party/remote:BUILD.output_vt100-0.1.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__parking_lot__0_12_1",
        url = "https://crates.io/api/v1/crates/parking_lot/0.12.1/download",
        type = "tar.gz",
        sha256 = "3742b2c103b9f06bc9fff0a37ff4912935851bee6d36f3c02bcc755bcfec228f",
        strip_prefix = "parking_lot-0.12.1",
        build_file = Label("//third_party/remote:BUILD.parking_lot-0.12.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__parking_lot_core__0_9_8",
        url = "https://crates.io/api/v1/crates/parking_lot_core/0.9.8/download",
        type = "tar.gz",
        sha256 = "93f00c865fe7cabf650081affecd3871070f26767e7b2070a3ffae14c654b447",
        strip_prefix = "parking_lot_core-0.9.8",
        build_file = Label("//third_party/remote:BUILD.parking_lot_core-0.9.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__percent_encoding__2_3_0",
        url = "https://crates.io/api/v1/crates/percent-encoding/2.3.0/download",
        type = "tar.gz",
        sha256 = "9b2a4787296e9989611394c33f193f676704af1686e70b8f8033ab5ba9a35a94",
        strip_prefix = "percent-encoding-2.3.0",
        build_file = Label("//third_party/remote:BUILD.percent-encoding-2.3.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pest__2_6_1",
        url = "https://crates.io/api/v1/crates/pest/2.6.1/download",
        type = "tar.gz",
        sha256 = "16833386b02953ca926d19f64af613b9bf742c48dcd5e09b32fbfc9740bf84e2",
        strip_prefix = "pest-2.6.1",
        build_file = Label("//third_party/remote:BUILD.pest-2.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pest_derive__2_6_1",
        url = "https://crates.io/api/v1/crates/pest_derive/2.6.1/download",
        type = "tar.gz",
        sha256 = "7763190f9406839f99e5197afee8c9e759969f7dbfa40ad3b8dbee8757b745b5",
        strip_prefix = "pest_derive-2.6.1",
        build_file = Label("//third_party/remote:BUILD.pest_derive-2.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pest_generator__2_6_1",
        url = "https://crates.io/api/v1/crates/pest_generator/2.6.1/download",
        type = "tar.gz",
        sha256 = "249061b22e99973da1f5f5f1410284419e283bb60b79255bf5f42a94b66a2e00",
        strip_prefix = "pest_generator-2.6.1",
        build_file = Label("//third_party/remote:BUILD.pest_generator-2.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pest_meta__2_6_1",
        url = "https://crates.io/api/v1/crates/pest_meta/2.6.1/download",
        type = "tar.gz",
        sha256 = "457c310cfc9cf3f22bc58901cc7f0d3410ac5d6298e432a4f9a6138565cb6df6",
        strip_prefix = "pest_meta-2.6.1",
        build_file = Label("//third_party/remote:BUILD.pest_meta-2.6.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__petgraph__0_6_3",
        url = "https://crates.io/api/v1/crates/petgraph/0.6.3/download",
        type = "tar.gz",
        sha256 = "4dd7d28ee937e54fe3080c91faa1c3a46c06de6252988a7f4592ba2310ef22a4",
        strip_prefix = "petgraph-0.6.3",
        build_file = Label("//third_party/remote:BUILD.petgraph-0.6.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pin_project__1_1_0",
        url = "https://crates.io/api/v1/crates/pin-project/1.1.0/download",
        type = "tar.gz",
        sha256 = "c95a7476719eab1e366eaf73d0260af3021184f18177925b07f54b30089ceead",
        strip_prefix = "pin-project-1.1.0",
        build_file = Label("//third_party/remote:BUILD.pin-project-1.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pin_project_internal__1_1_0",
        url = "https://crates.io/api/v1/crates/pin-project-internal/1.1.0/download",
        type = "tar.gz",
        sha256 = "39407670928234ebc5e6e580247dd567ad73a3578460c5990f9503df207e8f07",
        strip_prefix = "pin-project-internal-1.1.0",
        build_file = Label("//third_party/remote:BUILD.pin-project-internal-1.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pin_project_lite__0_2_9",
        url = "https://crates.io/api/v1/crates/pin-project-lite/0.2.9/download",
        type = "tar.gz",
        sha256 = "e0a7ae3ac2f1173085d398531c705756c94a4c56843785df85a60c1a0afac116",
        strip_prefix = "pin-project-lite-0.2.9",
        build_file = Label("//third_party/remote:BUILD.pin-project-lite-0.2.9.bazel"),
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
        name = "raze__pkg_config__0_3_27",
        url = "https://crates.io/api/v1/crates/pkg-config/0.3.27/download",
        type = "tar.gz",
        sha256 = "26072860ba924cbfa98ea39c8c19b4dd6a4a25423dbdf219c1eca91aa0cf6964",
        strip_prefix = "pkg-config-0.3.27",
        build_file = Label("//third_party/remote:BUILD.pkg-config-0.3.27.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ppv_lite86__0_2_17",
        url = "https://crates.io/api/v1/crates/ppv-lite86/0.2.17/download",
        type = "tar.gz",
        sha256 = "5b40af805b3121feab8a3c29f04d8ad262fa8e0561883e7653e024ae4479e6de",
        strip_prefix = "ppv-lite86-0.2.17",
        build_file = Label("//third_party/remote:BUILD.ppv-lite86-0.2.17.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__pretty_assertions__0_7_2",
        url = "https://crates.io/api/v1/crates/pretty_assertions/0.7.2/download",
        type = "tar.gz",
        sha256 = "1cab0e7c02cf376875e9335e0ba1da535775beb5450d21e1dffca068818ed98b",
        strip_prefix = "pretty_assertions-0.7.2",
        build_file = Label("//third_party/remote:BUILD.pretty_assertions-0.7.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__prettyplease__0_1_25",
        url = "https://crates.io/api/v1/crates/prettyplease/0.1.25/download",
        type = "tar.gz",
        sha256 = "6c8646e95016a7a6c4adea95bafa8a16baab64b583356217f2c85db4a39d9a86",
        strip_prefix = "prettyplease-0.1.25",
        build_file = Label("//third_party/remote:BUILD.prettyplease-0.1.25.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__proc_macro2__1_0_60",
        url = "https://crates.io/api/v1/crates/proc-macro2/1.0.60/download",
        type = "tar.gz",
        sha256 = "dec2b086b7a862cf4de201096214fa870344cf922b2b30c167badb3af3195406",
        strip_prefix = "proc-macro2-1.0.60",
        build_file = Label("//third_party/remote:BUILD.proc-macro2-1.0.60.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__prost__0_11_9",
        url = "https://crates.io/api/v1/crates/prost/0.11.9/download",
        type = "tar.gz",
        sha256 = "0b82eaa1d779e9a4bc1c3217db8ffbeabaae1dca241bf70183242128d48681cd",
        strip_prefix = "prost-0.11.9",
        build_file = Label("//third_party/remote:BUILD.prost-0.11.9.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__prost_build__0_11_9",
        url = "https://crates.io/api/v1/crates/prost-build/0.11.9/download",
        type = "tar.gz",
        sha256 = "119533552c9a7ffacc21e099c24a0ac8bb19c2a2a3f363de84cd9b844feab270",
        strip_prefix = "prost-build-0.11.9",
        build_file = Label("//third_party/remote:BUILD.prost-build-0.11.9.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__prost_derive__0_11_9",
        url = "https://crates.io/api/v1/crates/prost-derive/0.11.9/download",
        type = "tar.gz",
        sha256 = "e5d2d8d10f3c6ded6da8b05b5fb3b8a5082514344d56c9f871412d29b4e075b4",
        strip_prefix = "prost-derive-0.11.9",
        build_file = Label("//third_party/remote:BUILD.prost-derive-0.11.9.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__prost_types__0_11_9",
        url = "https://crates.io/api/v1/crates/prost-types/0.11.9/download",
        type = "tar.gz",
        sha256 = "213622a1460818959ac1181aaeb2dc9c7f63df720db7d788b3e24eacd1983e13",
        strip_prefix = "prost-types-0.11.9",
        build_file = Label("//third_party/remote:BUILD.prost-types-0.11.9.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__quote__1_0_28",
        url = "https://crates.io/api/v1/crates/quote/1.0.28/download",
        type = "tar.gz",
        sha256 = "1b9ab9c7eadfd8df19006f1cf1a4aed13540ed5cbc047010ece5826e10825488",
        strip_prefix = "quote-1.0.28",
        build_file = Label("//third_party/remote:BUILD.quote-1.0.28.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rand__0_8_5",
        url = "https://crates.io/api/v1/crates/rand/0.8.5/download",
        type = "tar.gz",
        sha256 = "34af8d1a0e25924bc5b7c43c079c942339d8f0a8b57c39049bef581b46327404",
        strip_prefix = "rand-0.8.5",
        build_file = Label("//third_party/remote:BUILD.rand-0.8.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rand_chacha__0_3_1",
        url = "https://crates.io/api/v1/crates/rand_chacha/0.3.1/download",
        type = "tar.gz",
        sha256 = "e6c10a63a0fa32252be49d21e7709d4d4baf8d231c2dbce1eaa8141b9b127d88",
        strip_prefix = "rand_chacha-0.3.1",
        build_file = Label("//third_party/remote:BUILD.rand_chacha-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rand_core__0_6_4",
        url = "https://crates.io/api/v1/crates/rand_core/0.6.4/download",
        type = "tar.gz",
        sha256 = "ec0be4795e2f6a28069bec0b5ff3e2ac9bafc99e6a9a7dc3547996c5c816922c",
        strip_prefix = "rand_core-0.6.4",
        build_file = Label("//third_party/remote:BUILD.rand_core-0.6.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__redox_syscall__0_2_16",
        url = "https://crates.io/api/v1/crates/redox_syscall/0.2.16/download",
        type = "tar.gz",
        sha256 = "fb5a58c1855b4b6819d59012155603f0b22ad30cad752600aadfcb695265519a",
        strip_prefix = "redox_syscall-0.2.16",
        build_file = Label("//third_party/remote:BUILD.redox_syscall-0.2.16.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__redox_syscall__0_3_5",
        url = "https://crates.io/api/v1/crates/redox_syscall/0.3.5/download",
        type = "tar.gz",
        sha256 = "567664f262709473930a4bf9e51bf2ebf3348f2e748ccc50dea20646858f8f29",
        strip_prefix = "redox_syscall-0.3.5",
        build_file = Label("//third_party/remote:BUILD.redox_syscall-0.3.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__redox_users__0_4_3",
        url = "https://crates.io/api/v1/crates/redox_users/0.4.3/download",
        type = "tar.gz",
        sha256 = "b033d837a7cf162d7993aded9304e30a83213c648b6e389db233191f891e5c2b",
        strip_prefix = "redox_users-0.4.3",
        build_file = Label("//third_party/remote:BUILD.redox_users-0.4.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__regex__1_8_4",
        url = "https://crates.io/api/v1/crates/regex/1.8.4/download",
        type = "tar.gz",
        sha256 = "d0ab3ca65655bb1e41f2a8c8cd662eb4fb035e67c3f78da1d61dffe89d07300f",
        strip_prefix = "regex-1.8.4",
        build_file = Label("//third_party/remote:BUILD.regex-1.8.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__regex_syntax__0_7_2",
        url = "https://crates.io/api/v1/crates/regex-syntax/0.7.2/download",
        type = "tar.gz",
        sha256 = "436b050e76ed2903236f032a59761c1eb99e1b0aead2c257922771dab1fc8c78",
        strip_prefix = "regex-syntax-0.7.2",
        build_file = Label("//third_party/remote:BUILD.regex-syntax-0.7.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__relative_path__1_8_0",
        url = "https://crates.io/api/v1/crates/relative-path/1.8.0/download",
        type = "tar.gz",
        sha256 = "4bf2521270932c3c7bed1a59151222bd7643c79310f2916f01925e1e16255698",
        strip_prefix = "relative-path-1.8.0",
        build_file = Label("//third_party/remote:BUILD.relative-path-1.8.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rusoto_core__0_48_0",
        url = "https://crates.io/api/v1/crates/rusoto_core/0.48.0/download",
        type = "tar.gz",
        sha256 = "1db30db44ea73551326269adcf7a2169428a054f14faf9e1768f2163494f2fa2",
        strip_prefix = "rusoto_core-0.48.0",
        build_file = Label("//third_party/remote:BUILD.rusoto_core-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rusoto_credential__0_48_0",
        url = "https://crates.io/api/v1/crates/rusoto_credential/0.48.0/download",
        type = "tar.gz",
        sha256 = "ee0a6c13db5aad6047b6a44ef023dbbc21a056b6dab5be3b79ce4283d5c02d05",
        strip_prefix = "rusoto_credential-0.48.0",
        build_file = Label("//third_party/remote:BUILD.rusoto_credential-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rusoto_mock__0_48_0",
        url = "https://crates.io/api/v1/crates/rusoto_mock/0.48.0/download",
        type = "tar.gz",
        sha256 = "5a384880f3c6d514e9499e6df75490bef5f6f39237bc24844e3933dfc09e9e55",
        strip_prefix = "rusoto_mock-0.48.0",
        build_file = Label("//third_party/remote:BUILD.rusoto_mock-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rusoto_s3__0_48_0",
        url = "https://crates.io/api/v1/crates/rusoto_s3/0.48.0/download",
        type = "tar.gz",
        sha256 = "7aae4677183411f6b0b412d66194ef5403293917d66e70ab118f07cc24c5b14d",
        strip_prefix = "rusoto_s3-0.48.0",
        build_file = Label("//third_party/remote:BUILD.rusoto_s3-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rusoto_signature__0_48_0",
        url = "https://crates.io/api/v1/crates/rusoto_signature/0.48.0/download",
        type = "tar.gz",
        sha256 = "a5ae95491c8b4847931e291b151127eccd6ff8ca13f33603eb3d0035ecb05272",
        strip_prefix = "rusoto_signature-0.48.0",
        build_file = Label("//third_party/remote:BUILD.rusoto_signature-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustc_version__0_4_0",
        url = "https://crates.io/api/v1/crates/rustc_version/0.4.0/download",
        type = "tar.gz",
        sha256 = "bfa0f585226d2e68097d4f95d113b15b83a82e819ab25717ec0590d9584ef366",
        strip_prefix = "rustc_version-0.4.0",
        build_file = Label("//third_party/remote:BUILD.rustc_version-0.4.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustix__0_37_20",
        url = "https://crates.io/api/v1/crates/rustix/0.37.20/download",
        type = "tar.gz",
        sha256 = "b96e891d04aa506a6d1f318d2771bcb1c7dfda84e126660ace067c9b474bb2c0",
        strip_prefix = "rustix-0.37.20",
        build_file = Label("//third_party/remote:BUILD.rustix-0.37.20.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__rustversion__1_0_12",
        url = "https://crates.io/api/v1/crates/rustversion/1.0.12/download",
        type = "tar.gz",
        sha256 = "4f3208ce4d8448b3f3e7d168a73f5e0c43a61e32930de3bceeccedb388b6bf06",
        strip_prefix = "rustversion-1.0.12",
        build_file = Label("//third_party/remote:BUILD.rustversion-1.0.12.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ryu__1_0_13",
        url = "https://crates.io/api/v1/crates/ryu/1.0.13/download",
        type = "tar.gz",
        sha256 = "f91339c0467de62360649f8d3e185ca8de4224ff281f66000de5eb2a77a79041",
        strip_prefix = "ryu-1.0.13",
        build_file = Label("//third_party/remote:BUILD.ryu-1.0.13.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__schannel__0_1_21",
        url = "https://crates.io/api/v1/crates/schannel/0.1.21/download",
        type = "tar.gz",
        sha256 = "713cfb06c7059f3588fb8044c0fad1d09e3c01d225e25b9220dbfdcf16dbb1b3",
        strip_prefix = "schannel-0.1.21",
        build_file = Label("//third_party/remote:BUILD.schannel-0.1.21.bazel"),
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
        name = "raze__security_framework__2_9_1",
        url = "https://crates.io/api/v1/crates/security-framework/2.9.1/download",
        type = "tar.gz",
        sha256 = "1fc758eb7bffce5b308734e9b0c1468893cae9ff70ebf13e7090be8dcbcc83a8",
        strip_prefix = "security-framework-2.9.1",
        build_file = Label("//third_party/remote:BUILD.security-framework-2.9.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__security_framework_sys__2_9_0",
        url = "https://crates.io/api/v1/crates/security-framework-sys/2.9.0/download",
        type = "tar.gz",
        sha256 = "f51d0c0d83bec45f16480d0ce0058397a69e48fcdc52d1dc8855fb68acbd31a7",
        strip_prefix = "security-framework-sys-2.9.0",
        build_file = Label("//third_party/remote:BUILD.security-framework-sys-2.9.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__semver__1_0_17",
        url = "https://crates.io/api/v1/crates/semver/1.0.17/download",
        type = "tar.gz",
        sha256 = "bebd363326d05ec3e2f532ab7660680f3b02130d780c299bca73469d521bc0ed",
        strip_prefix = "semver-1.0.17",
        build_file = Label("//third_party/remote:BUILD.semver-1.0.17.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__serde__1_0_164",
        url = "https://crates.io/api/v1/crates/serde/1.0.164/download",
        type = "tar.gz",
        sha256 = "9e8c8cf938e98f769bc164923b06dce91cea1751522f46f8466461af04c9027d",
        strip_prefix = "serde-1.0.164",
        build_file = Label("//third_party/remote:BUILD.serde-1.0.164.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__serde_derive__1_0_164",
        url = "https://crates.io/api/v1/crates/serde_derive/1.0.164/download",
        type = "tar.gz",
        sha256 = "d9735b638ccc51c28bf6914d90a2e9725b377144fc612c49a611fddd1b631d68",
        strip_prefix = "serde_derive-1.0.164",
        build_file = Label("//third_party/remote:BUILD.serde_derive-1.0.164.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__serde_json__1_0_97",
        url = "https://crates.io/api/v1/crates/serde_json/1.0.97/download",
        type = "tar.gz",
        sha256 = "bdf3bf93142acad5821c99197022e170842cdbc1c30482b98750c688c640842a",
        strip_prefix = "serde_json-1.0.97",
        build_file = Label("//third_party/remote:BUILD.serde_json-1.0.97.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__sha2__0_10_7",
        url = "https://crates.io/api/v1/crates/sha2/0.10.7/download",
        type = "tar.gz",
        sha256 = "479fb9d862239e610720565ca91403019f2f00410f1864c5aa7479b950a76ed8",
        strip_prefix = "sha2-0.10.7",
        build_file = Label("//third_party/remote:BUILD.sha2-0.10.7.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__sha2__0_9_9",
        url = "https://crates.io/api/v1/crates/sha2/0.9.9/download",
        type = "tar.gz",
        sha256 = "4d58a1e1bf39749807d89cf2d98ac2dfa0ff1cb3faa38fbb64dd88ac8013d800",
        strip_prefix = "sha2-0.9.9",
        build_file = Label("//third_party/remote:BUILD.sha2-0.9.9.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__shellexpand__2_1_2",
        url = "https://crates.io/api/v1/crates/shellexpand/2.1.2/download",
        type = "tar.gz",
        sha256 = "7ccc8076840c4da029af4f87e4e8daeb0fca6b87bbb02e10cb60b791450e11e4",
        strip_prefix = "shellexpand-2.1.2",
        build_file = Label("//third_party/remote:BUILD.shellexpand-2.1.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__shlex__1_1_0",
        url = "https://crates.io/api/v1/crates/shlex/1.1.0/download",
        type = "tar.gz",
        sha256 = "43b2853a4d09f215c24cc5489c992ce46052d359b5109343cbafbf26bc62f8a3",
        strip_prefix = "shlex-1.1.0",
        build_file = Label("//third_party/remote:BUILD.shlex-1.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__signal_hook_registry__1_4_1",
        url = "https://crates.io/api/v1/crates/signal-hook-registry/1.4.1/download",
        type = "tar.gz",
        sha256 = "d8229b473baa5980ac72ef434c4415e70c4b5e71b423043adb4ba059f89c99a1",
        strip_prefix = "signal-hook-registry-1.4.1",
        build_file = Label("//third_party/remote:BUILD.signal-hook-registry-1.4.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__slab__0_4_8",
        url = "https://crates.io/api/v1/crates/slab/0.4.8/download",
        type = "tar.gz",
        sha256 = "6528351c9bc8ab22353f9d776db39a20288e8d6c37ef8cfe3317cf875eecfc2d",
        strip_prefix = "slab-0.4.8",
        build_file = Label("//third_party/remote:BUILD.slab-0.4.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__smallvec__1_10_0",
        url = "https://crates.io/api/v1/crates/smallvec/1.10.0/download",
        type = "tar.gz",
        sha256 = "a507befe795404456341dfab10cef66ead4c041f62b8b11bbb92bffe5d0953e0",
        strip_prefix = "smallvec-1.10.0",
        build_file = Label("//third_party/remote:BUILD.smallvec-1.10.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__socket2__0_4_9",
        url = "https://crates.io/api/v1/crates/socket2/0.4.9/download",
        type = "tar.gz",
        sha256 = "64a4a911eed85daf18834cfaa86a79b7d266ff93ff5ba14005426219480ed662",
        strip_prefix = "socket2-0.4.9",
        build_file = Label("//third_party/remote:BUILD.socket2-0.4.9.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__static_assertions__1_1_0",
        url = "https://crates.io/api/v1/crates/static_assertions/1.1.0/download",
        type = "tar.gz",
        sha256 = "a2eb9349b6444b326872e140eb1cf5e7c522154d69e7a0ffb0fb81c06b37543f",
        strip_prefix = "static_assertions-1.1.0",
        build_file = Label("//third_party/remote:BUILD.static_assertions-1.1.0.bazel"),
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
        name = "raze__strsim__0_10_0",
        url = "https://crates.io/api/v1/crates/strsim/0.10.0/download",
        type = "tar.gz",
        sha256 = "73473c0e59e6d5812c5dfe2a064a6444949f089e20eec9a2e5506596494e4623",
        strip_prefix = "strsim-0.10.0",
        build_file = Label("//third_party/remote:BUILD.strsim-0.10.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__subtle__2_4_1",
        url = "https://crates.io/api/v1/crates/subtle/2.4.1/download",
        type = "tar.gz",
        sha256 = "6bdef32e8150c2a081110b42772ffe7d7c9032b606bc226c8260fd97e0976601",
        strip_prefix = "subtle-2.4.1",
        build_file = Label("//third_party/remote:BUILD.subtle-2.4.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__syn__1_0_109",
        url = "https://crates.io/api/v1/crates/syn/1.0.109/download",
        type = "tar.gz",
        sha256 = "72b64191b275b66ffe2469e8af2c1cfe3bafa67b529ead792a6d0160888b4237",
        strip_prefix = "syn-1.0.109",
        build_file = Label("//third_party/remote:BUILD.syn-1.0.109.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__syn__2_0_18",
        url = "https://crates.io/api/v1/crates/syn/2.0.18/download",
        type = "tar.gz",
        sha256 = "32d41677bcbe24c20c52e7c70b0d8db04134c5d1066bf98662e2871ad200ea3e",
        strip_prefix = "syn-2.0.18",
        build_file = Label("//third_party/remote:BUILD.syn-2.0.18.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__sync_wrapper__0_1_2",
        url = "https://crates.io/api/v1/crates/sync_wrapper/0.1.2/download",
        type = "tar.gz",
        sha256 = "2047c6ded9c721764247e62cd3b03c09ffc529b2ba5b10ec482ae507a4a70160",
        strip_prefix = "sync_wrapper-0.1.2",
        build_file = Label("//third_party/remote:BUILD.sync_wrapper-0.1.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tempfile__3_6_0",
        url = "https://crates.io/api/v1/crates/tempfile/3.6.0/download",
        type = "tar.gz",
        sha256 = "31c0432476357e58790aaa47a8efb0c5138f137343f3b5f23bd36a27e3b0a6d6",
        strip_prefix = "tempfile-3.6.0",
        build_file = Label("//third_party/remote:BUILD.tempfile-3.6.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__termcolor__1_2_0",
        url = "https://crates.io/api/v1/crates/termcolor/1.2.0/download",
        type = "tar.gz",
        sha256 = "be55cf8942feac5c765c2c993422806843c9a9a45d4d5c407ad6dd2ea95eb9b6",
        strip_prefix = "termcolor-1.2.0",
        build_file = Label("//third_party/remote:BUILD.termcolor-1.2.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__thiserror__1_0_40",
        url = "https://crates.io/api/v1/crates/thiserror/1.0.40/download",
        type = "tar.gz",
        sha256 = "978c9a314bd8dc99be594bc3c175faaa9794be04a5a5e153caba6915336cebac",
        strip_prefix = "thiserror-1.0.40",
        build_file = Label("//third_party/remote:BUILD.thiserror-1.0.40.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__thiserror_impl__1_0_40",
        url = "https://crates.io/api/v1/crates/thiserror-impl/1.0.40/download",
        type = "tar.gz",
        sha256 = "f9456a42c5b0d803c8cd86e73dd7cc9edd429499f37a3550d286d5e86720569f",
        strip_prefix = "thiserror-impl-1.0.40",
        build_file = Label("//third_party/remote:BUILD.thiserror-impl-1.0.40.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__time__0_1_45",
        url = "https://crates.io/api/v1/crates/time/0.1.45/download",
        type = "tar.gz",
        sha256 = "1b797afad3f312d1c66a56d11d0316f916356d11bd158fbc6ca6389ff6bf805a",
        strip_prefix = "time-0.1.45",
        build_file = Label("//third_party/remote:BUILD.time-0.1.45.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio__1_28_2",
        url = "https://crates.io/api/v1/crates/tokio/1.28.2/download",
        type = "tar.gz",
        sha256 = "94d7b1cfd2aa4011f2de74c2c4c63665e27a71006b0a192dcd2710272e73dfa2",
        strip_prefix = "tokio-1.28.2",
        build_file = Label("//third_party/remote:BUILD.tokio-1.28.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio_io_timeout__1_2_0",
        url = "https://crates.io/api/v1/crates/tokio-io-timeout/1.2.0/download",
        type = "tar.gz",
        sha256 = "30b74022ada614a1b4834de765f9bb43877f910cc8ce4be40e89042c9223a8bf",
        strip_prefix = "tokio-io-timeout-1.2.0",
        build_file = Label("//third_party/remote:BUILD.tokio-io-timeout-1.2.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio_macros__2_1_0",
        url = "https://crates.io/api/v1/crates/tokio-macros/2.1.0/download",
        type = "tar.gz",
        sha256 = "630bdcf245f78637c13ec01ffae6187cca34625e8c63150d424b59e55af2675e",
        strip_prefix = "tokio-macros-2.1.0",
        build_file = Label("//third_party/remote:BUILD.tokio-macros-2.1.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio_native_tls__0_3_1",
        url = "https://crates.io/api/v1/crates/tokio-native-tls/0.3.1/download",
        type = "tar.gz",
        sha256 = "bbae76ab933c85776efabc971569dd6119c580d8f5d448769dec1764bf796ef2",
        strip_prefix = "tokio-native-tls-0.3.1",
        build_file = Label("//third_party/remote:BUILD.tokio-native-tls-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio_stream__0_1_14",
        url = "https://crates.io/api/v1/crates/tokio-stream/0.1.14/download",
        type = "tar.gz",
        sha256 = "397c988d37662c7dda6d2208364a706264bf3d6138b11d436cbac0ad38832842",
        strip_prefix = "tokio-stream-0.1.14",
        build_file = Label("//third_party/remote:BUILD.tokio-stream-0.1.14.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio_util__0_6_10",
        url = "https://crates.io/api/v1/crates/tokio-util/0.6.10/download",
        type = "tar.gz",
        sha256 = "36943ee01a6d67977dd3f84a5a1d2efeb4ada3a1ae771cadfaa535d9d9fc6507",
        strip_prefix = "tokio-util-0.6.10",
        build_file = Label("//third_party/remote:BUILD.tokio-util-0.6.10.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tokio_util__0_7_8",
        url = "https://crates.io/api/v1/crates/tokio-util/0.7.8/download",
        type = "tar.gz",
        sha256 = "806fe8c2c87eccc8b3267cbae29ed3ab2d0bd37fca70ab622e46aaa9375ddb7d",
        strip_prefix = "tokio-util-0.7.8",
        build_file = Label("//third_party/remote:BUILD.tokio-util-0.7.8.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tonic__0_8_3",
        url = "https://crates.io/api/v1/crates/tonic/0.8.3/download",
        type = "tar.gz",
        sha256 = "8f219fad3b929bef19b1f86fbc0358d35daed8f2cac972037ac0dc10bbb8d5fb",
        strip_prefix = "tonic-0.8.3",
        build_file = Label("//third_party/remote:BUILD.tonic-0.8.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tonic_build__0_8_4",
        url = "https://crates.io/api/v1/crates/tonic-build/0.8.4/download",
        type = "tar.gz",
        sha256 = "5bf5e9b9c0f7e0a7c027dcfaba7b2c60816c7049171f679d99ee2ff65d0de8c4",
        strip_prefix = "tonic-build-0.8.4",
        build_file = Label("//third_party/remote:BUILD.tonic-build-0.8.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower__0_4_13",
        url = "https://crates.io/api/v1/crates/tower/0.4.13/download",
        type = "tar.gz",
        sha256 = "b8fa9be0de6cf49e536ce1851f987bd21a43b771b09473c3549a6c853db37c1c",
        strip_prefix = "tower-0.4.13",
        build_file = Label("//third_party/remote:BUILD.tower-0.4.13.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_layer__0_3_2",
        url = "https://crates.io/api/v1/crates/tower-layer/0.3.2/download",
        type = "tar.gz",
        sha256 = "c20c8dbed6283a09604c3e69b4b7eeb54e298b8a600d4d5ecb5ad39de609f1d0",
        strip_prefix = "tower-layer-0.3.2",
        build_file = Label("//third_party/remote:BUILD.tower-layer-0.3.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tower_service__0_3_2",
        url = "https://crates.io/api/v1/crates/tower-service/0.3.2/download",
        type = "tar.gz",
        sha256 = "b6bc1c9ce2b5135ac7f93c72918fc37feb872bdc6a5533a8b85eb4b86bfdae52",
        strip_prefix = "tower-service-0.3.2",
        build_file = Label("//third_party/remote:BUILD.tower-service-0.3.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tracing__0_1_37",
        url = "https://crates.io/api/v1/crates/tracing/0.1.37/download",
        type = "tar.gz",
        sha256 = "8ce8c33a8d48bd45d624a6e523445fd21ec13d3653cd51f681abf67418f54eb8",
        strip_prefix = "tracing-0.1.37",
        build_file = Label("//third_party/remote:BUILD.tracing-0.1.37.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tracing_attributes__0_1_25",
        url = "https://crates.io/api/v1/crates/tracing-attributes/0.1.25/download",
        type = "tar.gz",
        sha256 = "8803eee176538f94ae9a14b55b2804eb7e1441f8210b1c31290b3bccdccff73b",
        strip_prefix = "tracing-attributes-0.1.25",
        build_file = Label("//third_party/remote:BUILD.tracing-attributes-0.1.25.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tracing_core__0_1_31",
        url = "https://crates.io/api/v1/crates/tracing-core/0.1.31/download",
        type = "tar.gz",
        sha256 = "0955b8137a1df6f1a2e9a37d8a6656291ff0297c1a97c24e0d8425fe2312f79a",
        strip_prefix = "tracing-core-0.1.31",
        build_file = Label("//third_party/remote:BUILD.tracing-core-0.1.31.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__tracing_futures__0_2_5",
        url = "https://crates.io/api/v1/crates/tracing-futures/0.2.5/download",
        type = "tar.gz",
        sha256 = "97d095ae15e245a057c8e8451bab9b3ee1e1f68e9ba2b4fbc18d0ac5237835f2",
        strip_prefix = "tracing-futures-0.2.5",
        build_file = Label("//third_party/remote:BUILD.tracing-futures-0.2.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__try_lock__0_2_4",
        url = "https://crates.io/api/v1/crates/try-lock/0.2.4/download",
        type = "tar.gz",
        sha256 = "3528ecfd12c466c6f163363caf2d02a71161dd5e1cc6ae7b34207ea2d42d81ed",
        strip_prefix = "try-lock-0.2.4",
        build_file = Label("//third_party/remote:BUILD.try-lock-0.2.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__twox_hash__1_6_3",
        url = "https://crates.io/api/v1/crates/twox-hash/1.6.3/download",
        type = "tar.gz",
        sha256 = "97fee6b57c6a41524a810daee9286c02d7752c4253064d0b05472833a438f675",
        strip_prefix = "twox-hash-1.6.3",
        build_file = Label("//third_party/remote:BUILD.twox-hash-1.6.3.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__typenum__1_16_0",
        url = "https://crates.io/api/v1/crates/typenum/1.16.0/download",
        type = "tar.gz",
        sha256 = "497961ef93d974e23eb6f433eb5fe1b7930b659f06d12dec6fc44a8f554c0bba",
        strip_prefix = "typenum-1.16.0",
        build_file = Label("//third_party/remote:BUILD.typenum-1.16.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__ucd_trie__0_1_5",
        url = "https://crates.io/api/v1/crates/ucd-trie/0.1.5/download",
        type = "tar.gz",
        sha256 = "9e79c4d996edb816c91e4308506774452e55e95c3c9de07b6729e17e15a5ef81",
        strip_prefix = "ucd-trie-0.1.5",
        build_file = Label("//third_party/remote:BUILD.ucd-trie-0.1.5.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__unicode_ident__1_0_9",
        url = "https://crates.io/api/v1/crates/unicode-ident/1.0.9/download",
        type = "tar.gz",
        sha256 = "b15811caf2415fb889178633e7724bad2509101cde276048e013b9def5e51fa0",
        strip_prefix = "unicode-ident-1.0.9",
        build_file = Label("//third_party/remote:BUILD.unicode-ident-1.0.9.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__utf8parse__0_2_1",
        url = "https://crates.io/api/v1/crates/utf8parse/0.2.1/download",
        type = "tar.gz",
        sha256 = "711b9620af191e0cdc7468a8d14e709c3dcdb115b36f838e601583af800a370a",
        strip_prefix = "utf8parse-0.2.1",
        build_file = Label("//third_party/remote:BUILD.utf8parse-0.2.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__uuid__0_8_2",
        url = "https://crates.io/api/v1/crates/uuid/0.8.2/download",
        type = "tar.gz",
        sha256 = "bc5cf98d8186244414c848017f0e2676b3fcb46807f6668a97dfe67359a3c4b7",
        strip_prefix = "uuid-0.8.2",
        build_file = Label("//third_party/remote:BUILD.uuid-0.8.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__vcpkg__0_2_15",
        url = "https://crates.io/api/v1/crates/vcpkg/0.2.15/download",
        type = "tar.gz",
        sha256 = "accd4ea62f7bb7a82fe23066fb0957d48ef677f6eeb8215f372f52e48bb32426",
        strip_prefix = "vcpkg-0.2.15",
        build_file = Label("//third_party/remote:BUILD.vcpkg-0.2.15.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__version_check__0_9_4",
        url = "https://crates.io/api/v1/crates/version_check/0.9.4/download",
        type = "tar.gz",
        sha256 = "49874b5167b65d7193b8aba1567f5c7d93d001cafc34600cee003eda787e483f",
        strip_prefix = "version_check-0.9.4",
        build_file = Label("//third_party/remote:BUILD.version_check-0.9.4.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__want__0_3_1",
        url = "https://crates.io/api/v1/crates/want/0.3.1/download",
        type = "tar.gz",
        sha256 = "bfa7760aed19e106de2c7c0b581b509f2f25d3dacaf737cb82ac61bc6d760b0e",
        strip_prefix = "want-0.3.1",
        build_file = Label("//third_party/remote:BUILD.want-0.3.1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__wasi__0_10_0_wasi_snapshot_preview1",
        url = "https://crates.io/api/v1/crates/wasi/0.10.0+wasi-snapshot-preview1/download",
        type = "tar.gz",
        sha256 = "1a143597ca7c7793eff794def352d41792a93c481eb1042423ff7ff72ba2c31f",
        strip_prefix = "wasi-0.10.0+wasi-snapshot-preview1",
        build_file = Label("//third_party/remote:BUILD.wasi-0.10.0+wasi-snapshot-preview1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__wasi__0_11_0_wasi_snapshot_preview1",
        url = "https://crates.io/api/v1/crates/wasi/0.11.0+wasi-snapshot-preview1/download",
        type = "tar.gz",
        sha256 = "9c8d87e72b64a3b4db28d11ce29237c246188f4f51057d65a7eab63b7987e423",
        strip_prefix = "wasi-0.11.0+wasi-snapshot-preview1",
        build_file = Label("//third_party/remote:BUILD.wasi-0.11.0+wasi-snapshot-preview1.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__wasm_bindgen__0_2_87",
        url = "https://crates.io/api/v1/crates/wasm-bindgen/0.2.87/download",
        type = "tar.gz",
        sha256 = "7706a72ab36d8cb1f80ffbf0e071533974a60d0a308d01a5d0375bf60499a342",
        strip_prefix = "wasm-bindgen-0.2.87",
        build_file = Label("//third_party/remote:BUILD.wasm-bindgen-0.2.87.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__wasm_bindgen_backend__0_2_87",
        url = "https://crates.io/api/v1/crates/wasm-bindgen-backend/0.2.87/download",
        type = "tar.gz",
        sha256 = "5ef2b6d3c510e9625e5fe6f509ab07d66a760f0885d858736483c32ed7809abd",
        strip_prefix = "wasm-bindgen-backend-0.2.87",
        build_file = Label("//third_party/remote:BUILD.wasm-bindgen-backend-0.2.87.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__wasm_bindgen_macro__0_2_87",
        url = "https://crates.io/api/v1/crates/wasm-bindgen-macro/0.2.87/download",
        type = "tar.gz",
        sha256 = "dee495e55982a3bd48105a7b947fd2a9b4a8ae3010041b9e0faab3f9cd028f1d",
        strip_prefix = "wasm-bindgen-macro-0.2.87",
        build_file = Label("//third_party/remote:BUILD.wasm-bindgen-macro-0.2.87.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__wasm_bindgen_macro_support__0_2_87",
        url = "https://crates.io/api/v1/crates/wasm-bindgen-macro-support/0.2.87/download",
        type = "tar.gz",
        sha256 = "54681b18a46765f095758388f2d0cf16eb8d4169b639ab575a8f5693af210c7b",
        strip_prefix = "wasm-bindgen-macro-support-0.2.87",
        build_file = Label("//third_party/remote:BUILD.wasm-bindgen-macro-support-0.2.87.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__wasm_bindgen_shared__0_2_87",
        url = "https://crates.io/api/v1/crates/wasm-bindgen-shared/0.2.87/download",
        type = "tar.gz",
        sha256 = "ca6ad05a4870b2bf5fe995117d3728437bd27d7cd5f06f13c17443ef369775a1",
        strip_prefix = "wasm-bindgen-shared-0.2.87",
        build_file = Label("//third_party/remote:BUILD.wasm-bindgen-shared-0.2.87.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__which__4_4_0",
        url = "https://crates.io/api/v1/crates/which/4.4.0/download",
        type = "tar.gz",
        sha256 = "2441c784c52b289a054b7201fc93253e288f094e2f4be9058343127c4226a269",
        strip_prefix = "which-4.4.0",
        build_file = Label("//third_party/remote:BUILD.which-4.4.0.bazel"),
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
        name = "raze__windows__0_48_0",
        url = "https://crates.io/api/v1/crates/windows/0.48.0/download",
        type = "tar.gz",
        sha256 = "e686886bc078bc1b0b600cac0147aadb815089b6e4da64016cbd754b6342700f",
        strip_prefix = "windows-0.48.0",
        build_file = Label("//third_party/remote:BUILD.windows-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_sys__0_42_0",
        url = "https://crates.io/api/v1/crates/windows-sys/0.42.0/download",
        type = "tar.gz",
        sha256 = "5a3e1820f08b8513f676f7ab6c1f99ff312fb97b553d30ff4dd86f9f15728aa7",
        strip_prefix = "windows-sys-0.42.0",
        build_file = Label("//third_party/remote:BUILD.windows-sys-0.42.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_sys__0_48_0",
        url = "https://crates.io/api/v1/crates/windows-sys/0.48.0/download",
        type = "tar.gz",
        sha256 = "677d2418bec65e3338edb076e806bc1ec15693c5d0104683f2efe857f61056a9",
        strip_prefix = "windows-sys-0.48.0",
        build_file = Label("//third_party/remote:BUILD.windows-sys-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_targets__0_48_0",
        url = "https://crates.io/api/v1/crates/windows-targets/0.48.0/download",
        type = "tar.gz",
        sha256 = "7b1eb6f0cd7c80c79759c929114ef071b87354ce476d9d94271031c0497adfd5",
        strip_prefix = "windows-targets-0.48.0",
        build_file = Label("//third_party/remote:BUILD.windows-targets-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_aarch64_gnullvm__0_42_2",
        url = "https://crates.io/api/v1/crates/windows_aarch64_gnullvm/0.42.2/download",
        type = "tar.gz",
        sha256 = "597a5118570b68bc08d8d59125332c54f1ba9d9adeedeef5b99b02ba2b0698f8",
        strip_prefix = "windows_aarch64_gnullvm-0.42.2",
        build_file = Label("//third_party/remote:BUILD.windows_aarch64_gnullvm-0.42.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_aarch64_gnullvm__0_48_0",
        url = "https://crates.io/api/v1/crates/windows_aarch64_gnullvm/0.48.0/download",
        type = "tar.gz",
        sha256 = "91ae572e1b79dba883e0d315474df7305d12f569b400fcf90581b06062f7e1bc",
        strip_prefix = "windows_aarch64_gnullvm-0.48.0",
        build_file = Label("//third_party/remote:BUILD.windows_aarch64_gnullvm-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_aarch64_msvc__0_42_2",
        url = "https://crates.io/api/v1/crates/windows_aarch64_msvc/0.42.2/download",
        type = "tar.gz",
        sha256 = "e08e8864a60f06ef0d0ff4ba04124db8b0fb3be5776a5cd47641e942e58c4d43",
        strip_prefix = "windows_aarch64_msvc-0.42.2",
        build_file = Label("//third_party/remote:BUILD.windows_aarch64_msvc-0.42.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_aarch64_msvc__0_48_0",
        url = "https://crates.io/api/v1/crates/windows_aarch64_msvc/0.48.0/download",
        type = "tar.gz",
        sha256 = "b2ef27e0d7bdfcfc7b868b317c1d32c641a6fe4629c171b8928c7b08d98d7cf3",
        strip_prefix = "windows_aarch64_msvc-0.48.0",
        build_file = Label("//third_party/remote:BUILD.windows_aarch64_msvc-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_i686_gnu__0_42_2",
        url = "https://crates.io/api/v1/crates/windows_i686_gnu/0.42.2/download",
        type = "tar.gz",
        sha256 = "c61d927d8da41da96a81f029489353e68739737d3beca43145c8afec9a31a84f",
        strip_prefix = "windows_i686_gnu-0.42.2",
        build_file = Label("//third_party/remote:BUILD.windows_i686_gnu-0.42.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_i686_gnu__0_48_0",
        url = "https://crates.io/api/v1/crates/windows_i686_gnu/0.48.0/download",
        type = "tar.gz",
        sha256 = "622a1962a7db830d6fd0a69683c80a18fda201879f0f447f065a3b7467daa241",
        strip_prefix = "windows_i686_gnu-0.48.0",
        build_file = Label("//third_party/remote:BUILD.windows_i686_gnu-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_i686_msvc__0_42_2",
        url = "https://crates.io/api/v1/crates/windows_i686_msvc/0.42.2/download",
        type = "tar.gz",
        sha256 = "44d840b6ec649f480a41c8d80f9c65108b92d89345dd94027bfe06ac444d1060",
        strip_prefix = "windows_i686_msvc-0.42.2",
        build_file = Label("//third_party/remote:BUILD.windows_i686_msvc-0.42.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_i686_msvc__0_48_0",
        url = "https://crates.io/api/v1/crates/windows_i686_msvc/0.48.0/download",
        type = "tar.gz",
        sha256 = "4542c6e364ce21bf45d69fdd2a8e455fa38d316158cfd43b3ac1c5b1b19f8e00",
        strip_prefix = "windows_i686_msvc-0.48.0",
        build_file = Label("//third_party/remote:BUILD.windows_i686_msvc-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_x86_64_gnu__0_42_2",
        url = "https://crates.io/api/v1/crates/windows_x86_64_gnu/0.42.2/download",
        type = "tar.gz",
        sha256 = "8de912b8b8feb55c064867cf047dda097f92d51efad5b491dfb98f6bbb70cb36",
        strip_prefix = "windows_x86_64_gnu-0.42.2",
        build_file = Label("//third_party/remote:BUILD.windows_x86_64_gnu-0.42.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_x86_64_gnu__0_48_0",
        url = "https://crates.io/api/v1/crates/windows_x86_64_gnu/0.48.0/download",
        type = "tar.gz",
        sha256 = "ca2b8a661f7628cbd23440e50b05d705db3686f894fc9580820623656af974b1",
        strip_prefix = "windows_x86_64_gnu-0.48.0",
        build_file = Label("//third_party/remote:BUILD.windows_x86_64_gnu-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_x86_64_gnullvm__0_42_2",
        url = "https://crates.io/api/v1/crates/windows_x86_64_gnullvm/0.42.2/download",
        type = "tar.gz",
        sha256 = "26d41b46a36d453748aedef1486d5c7a85db22e56aff34643984ea85514e94a3",
        strip_prefix = "windows_x86_64_gnullvm-0.42.2",
        build_file = Label("//third_party/remote:BUILD.windows_x86_64_gnullvm-0.42.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_x86_64_gnullvm__0_48_0",
        url = "https://crates.io/api/v1/crates/windows_x86_64_gnullvm/0.48.0/download",
        type = "tar.gz",
        sha256 = "7896dbc1f41e08872e9d5e8f8baa8fdd2677f29468c4e156210174edc7f7b953",
        strip_prefix = "windows_x86_64_gnullvm-0.48.0",
        build_file = Label("//third_party/remote:BUILD.windows_x86_64_gnullvm-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_x86_64_msvc__0_42_2",
        url = "https://crates.io/api/v1/crates/windows_x86_64_msvc/0.42.2/download",
        type = "tar.gz",
        sha256 = "9aec5da331524158c6d1a4ac0ab1541149c0b9505fde06423b02f5ef0106b9f0",
        strip_prefix = "windows_x86_64_msvc-0.42.2",
        build_file = Label("//third_party/remote:BUILD.windows_x86_64_msvc-0.42.2.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__windows_x86_64_msvc__0_48_0",
        url = "https://crates.io/api/v1/crates/windows_x86_64_msvc/0.48.0/download",
        type = "tar.gz",
        sha256 = "1a515f5799fe4961cb532f983ce2b23082366b898e52ffbce459c86f67c8378a",
        strip_prefix = "windows_x86_64_msvc-0.48.0",
        build_file = Label("//third_party/remote:BUILD.windows_x86_64_msvc-0.48.0.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__xml_rs__0_8_14",
        url = "https://crates.io/api/v1/crates/xml-rs/0.8.14/download",
        type = "tar.gz",
        sha256 = "52839dc911083a8ef63efa4d039d1f58b5e409f923e44c80828f206f66e5541c",
        strip_prefix = "xml-rs-0.8.14",
        build_file = Label("//third_party/remote:BUILD.xml-rs-0.8.14.bazel"),
    )

    maybe(
        http_archive,
        name = "raze__zeroize__1_6_0",
        url = "https://crates.io/api/v1/crates/zeroize/1.6.0/download",
        type = "tar.gz",
        sha256 = "2a0956f1ba7c7909bfb66c2e9e4124ab6f6482560f6628b5aaeba39207c9aad9",
        strip_prefix = "zeroize-1.6.0",
        build_file = Label("//third_party/remote:BUILD.zeroize-1.6.0.bazel"),
    )
