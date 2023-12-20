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

load("@rules_cc//cc:defs.bzl", "cc_toolchain", "cc_toolchain_suite")
load(":armeabi_cc_toolchain_config.bzl", "armeabi_cc_toolchain_config")
load(":cc_toolchain_config.bzl", "cc_toolchain_config")

package(default_visibility = ["//visibility:public"])

licenses(["notice"])  # Apache 2.0

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
    name = "compiler_deps",
    srcs = glob(
        ["extra_tools/**"],
        allow_empty = True,
    ) + [":builtin_include_directory_paths"],
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
    coverage_compile_flags = ["--coverage"],
    coverage_link_flags = ["--coverage"],
    cpu = "k8",
    cxx_builtin_include_directories = [
        "/nix/store/1siqxvd4vlfl23pr1jg6cjsapnjwx5w1-clang-wrapper-17.0.6/resource-root/include",
        "/nix/store/jhi4wsbrxfscrf57k46d1lfq1v8d25kx-glibc-2.38-27-dev/include",
        "/nix/store/1siqxvd4vlfl23pr1jg6cjsapnjwx5w1-clang-wrapper-17.0.6/resource-root/share",
        "/nix/store/v966sq3k9q4543xsik6h74nsmqvk7mci-libcxx-17.0.6-dev/include/c++/v1",
        "/nix/store/5dzw89iz8firvr5rkx2ch56g77xfh07y-libcxxabi-17.0.6-dev/include/c++/v1",
    ],
    cxx_flags = ["-std=c++14"],
    dbg_compile_flags = ["-g"],
    host_system_name = "x86_64-unknown-linux-gnu",
    link_flags = [
        "-Wl,-no-as-needed",
        "-Wl,-z,relro,-z,now",
        "-B/nix/store/c12sgla3zb77xlfgws0yixm6kg46brbd-customClang/bin",
        "-B/nix/store/pw8r67xpm2i22cz5c83yz3cz72ii0ba1-binutils-wrapper-2.40/bin",
        "-L/nix/store/6kg2v6mfa2qy3fvm8hw70mglgzdi312i-libcxx-17.0.6/lib",
        "-L/nix/store/w38qhj29a43ahvdcdxgyk04lx231b7ry-libcxxabi-17.0.6/lib",
        "-L/nix/store/973cqkwjvdjh672idnxz17dbkfnsxavm-libunwind-17.0.6/lib",
        "-lc++",
        "-Wl,-rpath,/nix/store/6kg2v6mfa2qy3fvm8hw70mglgzdi312i-libcxx-17.0.6/lib,-rpath,/nix/store/w38qhj29a43ahvdcdxgyk04lx231b7ry-libcxxabi-17.0.6/lib,-rpath,/nix/store/973cqkwjvdjh672idnxz17dbkfnsxavm-libunwind-17.0.6/lib,-rpath,/nix/store/9y8pmvk8gdwwznmkzxa6pwyah52xy3nk-glibc-2.38-27/lib",
    ],
    link_libs = [
        "-lstdc++",
        "-lm",
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
    supports_start_end_lib = False,
    target_libc = "glibc_2.35",
    target_system_name = "local",
    tool_paths = {
        "ar": "/nix/store/pw8r67xpm2i22cz5c83yz3cz72ii0ba1-binutils-wrapper-2.40/bin/ar",
        "ld": "/nix/store/pw8r67xpm2i22cz5c83yz3cz72ii0ba1-binutils-wrapper-2.40/bin/ld",
        "llvm-cov": "None",
        "llvm-profdata": "None",
        "cpp": "/usr/bin/cpp",
        "gcc": "/nix/store/c12sgla3zb77xlfgws0yixm6kg46brbd-customClang/bin/customClang",
        "dwp": "/nix/store/pw8r67xpm2i22cz5c83yz3cz72ii0ba1-binutils-wrapper-2.40/bin/dwp",
        "gcov": "None",
        "nm": "/nix/store/pw8r67xpm2i22cz5c83yz3cz72ii0ba1-binutils-wrapper-2.40/bin/nm",
        "objcopy": "/nix/store/pw8r67xpm2i22cz5c83yz3cz72ii0ba1-binutils-wrapper-2.40/bin/objcopy",
        "objdump": "/nix/store/pw8r67xpm2i22cz5c83yz3cz72ii0ba1-binutils-wrapper-2.40/bin/objdump",
        "strip": "/nix/store/pw8r67xpm2i22cz5c83yz3cz72ii0ba1-binutils-wrapper-2.40/bin/strip",
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
