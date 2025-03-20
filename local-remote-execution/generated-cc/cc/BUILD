# Copyright 2016 The Bazel Authors. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# This becomes the BUILD file for @local_config_cc// under non-BSD unixes.

load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("@rules_cc//cc:defs.bzl", "cc_toolchain", "cc_toolchain_suite")
load(":armeabi_cc_toolchain_config.bzl", "armeabi_cc_toolchain_config")
load(":cc_toolchain_config.bzl", "cc_toolchain_config")

package(default_visibility = ["//visibility:public"])

licenses(["notice"])  # Apache 2.0

cc_library(name = "empty_lib")

# Label flag for extra libraries to be linked into every binary.
# TODO(bazel-team): Support passing flag multiple times to build a list.
label_flag(
    name = "link_extra_libs",
    build_setting_default = ":empty_lib",
)

# The final extra library to be linked into every binary target. This collects
# the above flag, but may also include more libraries depending on config.
cc_library(
    name = "link_extra_lib",
    deps = [
        ":link_extra_libs",
    ],
)

cc_library(
    name = "malloc",
)

filegroup(
    name = "empty",
    srcs = [],
)

filegroup(
    name = "cc_wrapper",
    srcs = ["cc_wrapper.sh"],
)

filegroup(
    name = "validate_static_library",
    srcs = ["validate_static_library.sh"],
)

filegroup(
    name = "compiler_deps",
    srcs = glob(
        ["extra_tools/**"],
        allow_empty = True,
    ) + [
        ":builtin_include_directory_paths",
        ":cc_wrapper",
        ":validate_static_library",
    ],
)

# This is the entry point for --crosstool_top.  Toolchains are found
# by lopping off the name of --crosstool_top and searching for
# the "${CPU}" entry in the toolchains attribute.
cc_toolchain_suite(
    name = "toolchain",
    toolchains = {
        "k8|clang": ":cc-compiler-k8",
        "k8": ":cc-compiler-k8",
        "armeabi-v7a|compiler": ":cc-compiler-armeabi-v7a",
        "armeabi-v7a": ":cc-compiler-armeabi-v7a",
    },
)

cc_toolchain(
    name = "cc-compiler-k8",
    all_files = ":compiler_deps",
    ar_files = ":compiler_deps",
    as_files = ":compiler_deps",
    compiler_files = ":compiler_deps",
    dwp_files = ":empty",
    linker_files = ":compiler_deps",
    module_map = ":module.modulemap",
    objcopy_files = ":empty",
    strip_files = ":empty",
    supports_header_parsing = 1,
    supports_param_files = 1,
    toolchain_config = ":local",
    toolchain_identifier = "local",
)

cc_toolchain_config(
    name = "local",
    abi_libc_version = "glibc_2.35",
    abi_version = "local",
    compile_flags = [
        "-fstack-protector",
        "-Wall",
        "-Wthread-safety",
        "-Wself-assign",
        "-Wunused-but-set-parameter",
        "-Wno-free-nonheap-object",
        "-fcolor-diagnostics",
        "-fno-omit-frame-pointer",
    ],
    compiler = "clang",
    conly_flags = [],
    coverage_compile_flags = ["--coverage"],
    coverage_link_flags = ["--coverage"],
    cpu = "k8",
    cxx_builtin_include_directories = [
        "/nix/store/i45c42749c7rkbv4idawbr9m4phi2c6h-clang-wrapper-19.1.7/resource-root/include",
        "/nix/store/5djq7mrpqv8kzn2xi22y5d8ww7rsix82-glibc-2.40-66-dev/include",
        "/nix/store/i45c42749c7rkbv4idawbr9m4phi2c6h-clang-wrapper-19.1.7/resource-root/share",
        "/nix/store/47jmi5pvn8xh447nz99n3nicy9fi831v-libcxx-19.1.7-dev/include/c++/v1",
        "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk",
    ],
    cxx_flags = ["-std=c++14"],
    dbg_compile_flags = ["-g"],
    extra_flags_per_feature = {"use_module_maps": [
        "-Xclang",
        "-fno-cxx-modules",
    ]},
    host_system_name = "x86_64-unknown-linux-gnu",
    link_flags = [
        "-fuse-ld=/nix/store/fj1xilxbnb4bll73hw273vx999n22rwi-llvm-binutils-wrapper-19.1.7/bin/ld.mold",
        "-B/nix/store/6dysvsc6y6yq3dii7lbixbkan6pmi58x-customClang/bin",
        "-Wl,-no-as-needed",
        "-Wl,-z,relro,-z,now",
    ],
    link_libs = [
        "-Wl,--push-state,-as-needed",
        "-lstdc++",
        "-Wl,--pop-state",
        "-Wl,--push-state,-as-needed",
        "-lm",
        "-Wl,--pop-state",
    ],
    opt_compile_flags = [
        "-g0",
        "-O2",
        "-D_FORTIFY_SOURCE=1",
        "-DNDEBUG",
        "-ffunction-sections",
        "-fdata-sections",
    ],
    opt_link_flags = ["-Wl,--gc-sections"],
    supports_start_end_lib = True,
    target_libc = "glibc_2.35",
    target_system_name = "local",
    tool_paths = {
        "ar": "/nix/store/fj1xilxbnb4bll73hw273vx999n22rwi-llvm-binutils-wrapper-19.1.7/bin/ar",
        "ld": "/nix/store/fj1xilxbnb4bll73hw273vx999n22rwi-llvm-binutils-wrapper-19.1.7/bin/ld",
        "llvm-cov": "None",
        "llvm-profdata": "None",
        "cpp": "/usr/bin/cpp",
        "gcc": "/nix/store/6dysvsc6y6yq3dii7lbixbkan6pmi58x-customClang/bin/customClang",
        "dwp": "/nix/store/fj1xilxbnb4bll73hw273vx999n22rwi-llvm-binutils-wrapper-19.1.7/bin/dwp",
        "gcov": "None",
        "nm": "/nix/store/fj1xilxbnb4bll73hw273vx999n22rwi-llvm-binutils-wrapper-19.1.7/bin/nm",
        "objcopy": "/nix/store/fj1xilxbnb4bll73hw273vx999n22rwi-llvm-binutils-wrapper-19.1.7/bin/objcopy",
        "objdump": "/nix/store/fj1xilxbnb4bll73hw273vx999n22rwi-llvm-binutils-wrapper-19.1.7/bin/objdump",
        "strip": "/nix/store/fj1xilxbnb4bll73hw273vx999n22rwi-llvm-binutils-wrapper-19.1.7/bin/strip",
        "c++filt": "/nix/store/fj1xilxbnb4bll73hw273vx999n22rwi-llvm-binutils-wrapper-19.1.7/bin/c++filt",
        "parse_headers": "cc_wrapper.sh",
        "validate_static_library": "validate_static_library.sh",
    },
    toolchain_identifier = "local",
    unfiltered_compile_flags = [
        "-no-canonical-prefixes",
        "-Wno-builtin-macro-redefined",
        "-D__DATE__=\"redacted\"",
        "-D__TIMESTAMP__=\"redacted\"",
        "-D__TIME__=\"redacted\"",
    ],
)

# Android tooling requires a default toolchain for the armeabi-v7a cpu.
cc_toolchain(
    name = "cc-compiler-armeabi-v7a",
    all_files = ":empty",
    ar_files = ":empty",
    as_files = ":empty",
    compiler_files = ":empty",
    dwp_files = ":empty",
    linker_files = ":empty",
    objcopy_files = ":empty",
    strip_files = ":empty",
    supports_param_files = 1,
    toolchain_config = ":stub_armeabi-v7a",
    toolchain_identifier = "stub_armeabi-v7a",
)

armeabi_cc_toolchain_config(name = "stub_armeabi-v7a")
