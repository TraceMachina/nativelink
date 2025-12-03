{
  nativelink,
  mongodb,
  wait4x,
  bazelisk,
  writeShellScriptBin,
}:
writeShellScriptBin "mongodb-with-nativelink-test" ''
  set -euo pipefail

  cleanup() {
    local pids=$(jobs -pr)
    [ -n "$pids" ] && kill $pids
  }
  trap "cleanup" INT QUIT TERM EXIT

  TMPDIR=$HOME/.cache/nativelink/
  mkdir -p "$TMPDIR"

  MONGO_DATA_DIR="''${TMPDIR}mongo-data"
  rm -Rf "$MONGO_DATA_DIR"
  mkdir -p "$MONGO_DATA_DIR"

  ${mongodb}/bin/mongod --dbpath "$MONGO_DATA_DIR" 2>&1 | tee -i integration_tests/mongo/mongo.log &
  ${wait4x}/bin/wait4x tcp localhost:27017
  ${nativelink}/bin/nativelink -- integration_tests/mongo/mongo.json5 2>&1 | tee -i integration_tests/mongo/nativelink.log &
  ${wait4x}/bin/wait4x tcp localhost:50051

  if [[ $OSTYPE == "darwin"* ]]; then
      CACHE_DIR=$(mktemp -d "''${TMPDIR}mongo-integration-test")
  else
      echo "Assumes Linux/WSL"
      CACHE_DIR=$(mktemp -d --tmpdir="$TMPDIR" --suffix="-mongo-integration-test")
  fi
  BAZEL_CACHE_DIR="$CACHE_DIR/bazel"
  rm -Rf BAZEL_CACHE_DIR

  ${bazelisk}/bin/bazelisk --output_base="$BAZEL_CACHE_DIR" clean --expunge
  bazel_output=$(${bazelisk}/bin/bazelisk --output_base="$BAZEL_CACHE_DIR" test --config self_test //:dummy_test 2>&1 | tee -i integration_tests/mongo/bazel-mongo.log)
  ${bazelisk}/bin/bazelisk shutdown

  case $bazel_output in
    *"1 test passes"* )
      echo "Saw a successful bazel+mongo build"
    ;;
    *)
      echo 'Failed mongo build:'
      echo $bazel_output
      exit 1
    ;;
  esac

  nativelink_output=$(cat integration_tests/mongo/nativelink.log)

  case $nativelink_output in
    *"ERROR"* )
      echo "Error in nativelink build"
      exit 1
    ;;
    *)
      echo 'Successful nativelink build'
    ;;
  esac
''
