{
  nativelink,
  writeShellScriptBin,
  bazelisk,
}:
writeShellScriptBin "rbe-toolchain-test" ''
  set -uo pipefail

  cleanup() {
    local pids=$(jobs -pr)
    [ -n "$pids" ] && kill $pids
  }
  trap "cleanup" INT QUIT TERM EXIT

  NO_COLOR=true ${nativelink}/bin/nativelink -- toolchain-examples/nativelink-config.json5 | tee -i toolchain-examples/nativelink.log &

  CORE_BAZEL_ARGS="--check_direct_dependencies=error --remote_cache=grpc://localhost:50051 --remote_executor=grpc://localhost:50051"

  CPU_TYPE=$(uname -m)

  if [[ "$CPU_TYPE" == 'x86_64' ]]; then
    PLATFORM='amd64'
  else
    PLATFORM='arm64'
  fi

  LLVM_PLATFORM="--platforms=@toolchains_llvm//platforms:linux-''${CPU_TYPE}"
  ZIG_PLATFORM="--platforms @zig_sdk//platform:linux_''${PLATFORM}"

  # As per https://nativelink.com/docs/rbe/remote-execution-examples#minimal-example-targets
  COMMANDS=("test //cpp $ZIG_PLATFORM"
            "test //cpp --config=llvm $LLVM_PLATFORM"
            "test //python"
            "test //go $ZIG_PLATFORM"
            # "test //rust $ZIG_PLATFORM" # rules_rust isn't RBE-compatible
            "test //java:HelloWorld --config=java"
            "build @curl//... $ZIG_PLATFORM"
            "build @zstd//... $ZIG_PLATFORM"
            # "test @abseil-cpp//... $ZIG_PLATFORM" # Buggy build due to google_benchmark errors
            "test @abseil-py//..."
            "test @circl//... $ZIG_PLATFORM"
            )

  echo "" > toolchain-examples/cmd.log
  for cmd in "''${COMMANDS[@]}"
  do
    FULL_CMD="${bazelisk}/bin/bazelisk $cmd $CORE_BAZEL_ARGS"
    echo $FULL_CMD
    echo -e \\n$FULL_CMD\\n >> toolchain-examples/cmd.log
    cmd_output=$(cd toolchain-examples && eval "$FULL_CMD" 2>&1 | tee -ai cmd.log)
    cmd_exit_code=$?
    case $cmd_exit_code in
      0 )
        echo "Saw a successful $cmd build"
      ;;
      *)
        echo "Failed $cmd build:"
        echo $cmd_output
        exit 1
      ;;
    esac
  done

  nativelink_output=$(cat toolchain-examples/nativelink.log)

  case $nativelink_output in
    *"ERROR "* )
      echo "Error in nativelink build"
      exit 1
    ;;
    *)
      echo 'Successful nativelink build'
    ;;
  esac
''
