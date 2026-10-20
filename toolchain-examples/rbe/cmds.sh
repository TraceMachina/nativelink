# shellcheck shell=bash
# Doesn't get run directly. See list.nix and test.nix in this folder for usages

CPU_TYPE=$(uname -m)

if [[ $CPU_TYPE == 'x86_64' ]]; then
    PLATFORM='amd64'
else
    PLATFORM='arm64'
fi

LLVM_PLATFORM="--config=llvm --platforms=@toolchains_llvm//platforms:linux-''${CPU_TYPE}"
ZIG_PLATFORM="--config=zig-cc --platforms @zig_sdk//platform:linux_''${PLATFORM}"
RUST_PLATFORM="--platforms=//platforms:linux_''${PLATFORM}_gnu_2_28 --host_platform=@rules_rs//:local_gnu_platform --extra_execution_platforms=@rules_rs//:local_gnu_platform"

# As per https://nativelink.com/docs/rbe/remote-execution-examples#minimal-example-targets
declare -A COMMANDS
# shellcheck disable=SC2034
COMMANDS=(
    [cpp-zig]="test //cpp $ZIG_PLATFORM"
    [cpp-llvm]="test //cpp $LLVM_PLATFORM"
    [python]="test //python"
    [go]="test //go $ZIG_PLATFORM"
    [rust]="test //rust $RUST_PLATFORM"
    [java]="test //java:HelloWorld --config=java"
    [curl]="build @curl//... $ZIG_PLATFORM"
    [zstd]="build @zstd//... $ZIG_PLATFORM"
    [abseil-py]="test @abseil-py//..."
    [circl]="test @circl//... $ZIG_PLATFORM"
)
# "test @abseil-cpp//... $ZIG_PLATFORM" # Buggy build due to google_benchmark errors
