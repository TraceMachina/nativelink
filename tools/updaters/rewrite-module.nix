{
  python-with-requests,
  writeShellScriptBin,
}:
writeShellScriptBin "update-module-hashes" ''
  set -uo pipefail

  ${python-with-requests}/bin/python tools/updaters/rewrite-module.py MODULE.bazel
''
