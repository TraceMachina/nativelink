{
  nativelink,
  buck2,
  writeShellScriptBin,
  coreutils,
  diffutils,
}:
writeShellScriptBin "buck2-with-nativelink-test" ''
  set -uo pipefail

  cleanup() {
    local pids=$(jobs -pr)
    [ -n "$pids" ] && kill $pids
  }
  trap "cleanup" INT QUIT TERM EXIT

  ${nativelink}/bin/nativelink -- integration_tests/buck2/buck2_cas.json5 | tee -i integration_tests/buck2/nativelink.log &

  cp integration_tests/buck2/tests/defs.bzl integration_tests/buck2/tests/defs.bzl.original
  sed -i -e 's#cat #${coreutils}/bin/cat #' integration_tests/buck2/tests/defs.bzl
  sed -i -e 's#diff #${diffutils}/bin/diff #' integration_tests/buck2/tests/defs.bzl

  buck2_output=$(cd integration_tests/buck2 && BUCK_NO_INTERACTIVE_CONSOLE=false BUCK_CONSOLE=simplenotty ${buck2}/bin/buck2 build //... 2>&1 | tee -i buck2.log)

  ${buck2}/bin/buck2 killall

  mv integration_tests/buck2/tests/defs.bzl.original integration_tests/buck2/tests/defs.bzl

  echo "Buck2 log"
  echo "---"
  cat integration_tests/buck2/buck2.log
  echo "---"

  case $buck2_output in
    *"BUILD SUCCEEDED"* )
      echo "Saw a successful buck2 build"
    ;;
    *)
      echo 'Failed buck2 build'
      exit 1
    ;;
  esac

  nativelink_output=$(cat integration_tests/buck2/nativelink.log)

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
