{
  nativelink,
  buildstream,
  buildbox,
  writeShellScriptBin,
}:
writeShellScriptBin "buildstream-with-nativelink-test" ''
  set -uo pipefail

  cleanup() {
    local pids=$(jobs -pr)
    [ -n "$pids" ] && kill $pids
  }
  trap "cleanup" INT QUIT TERM EXIT

  ${nativelink}/bin/nativelink -- integration_tests/buildstream/buildstream_cas.json5 &

  # TODO(palfrey): PATH is workaround for https://github.com/NixOS/nixpkgs/issues/248000#issuecomment-2934704963
  bst_output="$(cd integration_tests/buildstream && PATH=${buildbox}/bin:$PATH ${buildstream}/bin/bst -c buildstream.conf build hello.bst 2>&1 | tee -i buildstream.log)"

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
