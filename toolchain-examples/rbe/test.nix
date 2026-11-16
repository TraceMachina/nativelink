{
  nativelink,
  writeShellScriptBin,
  rbe-toolchain-cmd,
  bazel-retry,
  bazel,
}:
writeShellScriptBin "rbe-toolchain-test" ''
  set -uo pipefail

  ${rbe-toolchain-cmd}

  if [ $# -eq 0 ]
  then
    echo "No arguments supplied"
    keys=$(echo ''${!COMMANDS[@]} | xargs -n1 | sort | tr '\n' ' ')
    echo "Need list, all, or one of $keys"
    exit 1
  fi

  cleanup() {
    local pids=$(jobs -pr)
    [ -n "$pids" ] && kill $pids
  }
  trap "cleanup" INT QUIT TERM EXIT

  NO_COLOR=true ${nativelink}/bin/nativelink -- toolchain-examples/nativelink-config.json5 | tee -i toolchain-examples/nativelink.log &

  # --noshow_progress / --show_result=0 quiet the per-action progress stream and
  # the built-target listing so the RBE example logs stay scannable. Raw flags
  # (not --config=nl-quiet) because these commands run in the toolchain-examples
  # workspace, which does not define that config; and no --test_summary because
  # some commands here are `build` (curl, zstd), where it is not a valid option.
  CORE_BAZEL_ARGS="--check_direct_dependencies=error --remote_cache=grpc://localhost:50051 --remote_executor=grpc://localhost:50051 --noshow_progress --show_result=0"

  echo "" > toolchain-examples/cmd.log

  run_cmd() {
    cmd=$1
    FULL_CMD="PATH=${bazel}/bin:$PATH ${bazel-retry}/bin/bazel-retry $cmd $CORE_BAZEL_ARGS"
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
