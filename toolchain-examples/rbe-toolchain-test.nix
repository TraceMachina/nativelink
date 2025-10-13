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

  CORE_BAZEL_ARGS="${bazelisk}/bin/bazelisk build --check_direct_dependencies=error --remote_cache=grpc://localhost:50051 --remote_executor=grpc://localhost:50051"

  # As per https://nativelink.com/docs/rbe/remote-execution-examples#minimal-example-targets
  COMMANDS=("//cpp --config=zig-cc"
            "//cpp --config=llvm"
            "//python"
            "//go --config=zig-cc"
            # "//rust --config=zig-cc" # rules_rust isn't RBE-compatible
            "//java:HelloWorld --config=java"
            "@curl//... --config=zig-cc"
            "@zstd//... --config=zig-cc"
            # "@abseil-cpp//... --config=zig-cc" # Buggy build due to google_benchmark errors
            "@abseil-py//..."
            "@circl//... --config=zig-cc"
            )

  echo "" > toolchain-examples/cmd.log
  for cmd in "''${COMMANDS[@]}"
  do
    FULL_CMD="$CORE_BAZEL_ARGS $cmd"
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
