{
  nativelink,
  writeShellScriptBin,
  bazelisk,
  jo,
}:
writeShellScriptBin "rbe-toolchain-test" ''
  set -uo pipefail

  CPU_TYPE=$(uname -m)

  if [[ "$CPU_TYPE" == 'x86_64' ]]; then
    PLATFORM='amd64'
  else
    PLATFORM='arm64'
  fi

  LLVM_PLATFORM="--config=llvm --platforms=@toolchains_llvm//platforms:linux-''${CPU_TYPE}"
  ZIG_PLATFORM="--config=zig-cc --platforms @zig_sdk//platform:linux_''${PLATFORM}"
  RUST_ZIG_PLATFORM="--config=zig-cc --platforms=//platforms:linux_''${PLATFORM}_gnu_2_28 --host_platform=@rules_rs//:local_gnu_platform --extra_execution_platforms=@rules_rs//:local_gnu_platform"

  # As per https://nativelink.com/docs/rbe/remote-execution-examples#minimal-example-targets
  declare -A COMMANDS
  COMMANDS=(
    [cpp-zig]="test //cpp $ZIG_PLATFORM"
    [cpp-llvm]="test //cpp $LLVM_PLATFORM"
    [python]="test //python"
    [go]="test //go $ZIG_PLATFORM"
    [rust]="test //rust $RUST_ZIG_PLATFORM"
    [java]="test //java:HelloWorld --config=java"
    [curl]="build @curl//... $ZIG_PLATFORM"
    [zstd]="build @zstd//... $ZIG_PLATFORM"
    [abseil-py]="test @abseil-py//..."
    [circl]="test @circl//... $ZIG_PLATFORM"
  )
  # "test @abseil-cpp//... $ZIG_PLATFORM" # Buggy build due to google_benchmark errors

  if [ $# -eq 0 ]
  then
    echo "No arguments supplied"
    keys=$(echo ''${!COMMANDS[@]} | xargs -n1 | sort | tr '\n' ' ')
    echo "Need list, all, or one of $keys"
    exit 1
  fi

  if [ $1 == "list" ]
  then
    ${jo}/bin/jo -a "''${!COMMANDS[@]}"
    exit 0
  fi

  cleanup() {
    local pids=$(jobs -pr)
    [ -n "$pids" ] && kill $pids
  }
  trap "cleanup" INT QUIT TERM EXIT

  NO_COLOR=true ${nativelink}/bin/nativelink -- toolchain-examples/nativelink-config.json5 | tee -i toolchain-examples/nativelink.log &

  CORE_BAZEL_ARGS="--check_direct_dependencies=error --remote_cache=grpc://localhost:50051 --remote_executor=grpc://localhost:50051"

  echo "" > toolchain-examples/cmd.log

  run_cmd() {
    cmd=$1
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
  }

  if [ $1 == "all" ]
  then
    for cmd in "''${COMMANDS[@]}"
    do
      run_cmd "$cmd"
    done
  else
    cmd=''${COMMANDS[$1]:-}
    if [ -z "$cmd" ]
    then
      echo "Invalid command: $1"
      exit 1
    fi
    run_cmd "$cmd"
  fi

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
