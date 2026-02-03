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
        "/nix/store/z5yldddi6dh1dibwxijzjwl7f3c0ic4g-clang-wrapper-21.1.8/resource-root/include",
        "/nix/store/rwalsamz4246k8f1zzxa54qx7w3fbzdg-glibc-2.42-47-dev/include",
        "/nix/store/z5yldddi6dh1dibwxijzjwl7f3c0ic4g-clang-wrapper-21.1.8/resource-root/share",
        "/nix/store/nca9mnfn0xa414xb7708vkasr0270y08-libcxx-21.1.8-dev/include/c++/v1",
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
        "-fuse-ld=/nix/store/ck7fyp44vvsass6akvg633xv125a1kkb-llvm-binutils-wrapper-21.1.8/bin/ld.mold",
        "-B/nix/store/ddcvd6g65207r5iw7z20lk6whyfr642y-customClang/bin",
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
        "ar": "/nix/store/ck7fyp44vvsass6akvg633xv125a1kkb-llvm-binutils-wrapper-21.1.8/bin/ar",
        "ld": "/nix/store/ck7fyp44vvsass6akvg633xv125a1kkb-llvm-binutils-wrapper-21.1.8/bin/ld",
        "llvm-cov": "None",
        "llvm-profdata": "None",
        "cpp": "/usr/bin/cpp",
        "gcc": "/nix/store/ddcvd6g65207r5iw7z20lk6whyfr642y-customClang/bin/customClang",
        "dwp": "/nix/store/ck7fyp44vvsass6akvg633xv125a1kkb-llvm-binutils-wrapper-21.1.8/bin/dwp",
        "gcov": "None",
        "nm": "/nix/store/ck7fyp44vvsass6akvg633xv125a1kkb-llvm-binutils-wrapper-21.1.8/bin/nm",
        "objcopy": "/nix/store/ck7fyp44vvsass6akvg633xv125a1kkb-llvm-binutils-wrapper-21.1.8/bin/objcopy",
        "objdump": "/nix/store/ck7fyp44vvsass6akvg633xv125a1kkb-llvm-binutils-wrapper-21.1.8/bin/objdump",
        "strip": "/nix/store/ck7fyp44vvsass6akvg633xv125a1kkb-llvm-binutils-wrapper-21.1.8/bin/strip",
        "c++filt": "/nix/store/ck7fyp44vvsass6akvg633xv125a1kkb-llvm-binutils-wrapper-21.1.8/bin/c++filt",
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
