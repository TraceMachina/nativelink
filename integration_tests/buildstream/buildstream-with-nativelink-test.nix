{
  nativelink,
  bst,
  writeShellScriptBin,
}:
writeShellScriptBin "buildstream-with-nativelink-test" ''
  set -xuo pipefail

  cleanup() {
    local pids=$(jobs -pr)
    [ -n "$pids" ] && kill $pids
  }
  trap "cleanup" INT QUIT TERM EXIT

  ${nativelink}/bin/nativelink -- integration_tests/buildstream/buildstream_cas.json5 &

  bst_output="$(cd integration_tests/buildstream && ${bst}/bin/bst -c buildstream.conf build hello.bst 2>&1 | tee -i buildstream.log)"

  case $bst_output in
    *"SUCCESS Build"* )
      echo "Saw a successful buildstream build"
    ;;
    *)
      echo 'Failed buildstream build:'
      echo $bst_output
      exit 1
    ;;
  esac
''
