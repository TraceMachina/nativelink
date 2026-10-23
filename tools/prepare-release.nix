{
  git,
  git-cliff,
  python3,
  taplo,
  writeShellScriptBin,
}:
writeShellScriptBin "prepare-release" ''
  set -euo pipefail

  export PATH="${git}/bin:$PATH"
  export GIT_CLIFF_BIN="${git-cliff}/bin/git-cliff"
  export TAPLO_BIN="${taplo}/bin/taplo"

  ROOT="$(${git}/bin/git rev-parse --show-toplevel)"
  exec "${python3}/bin/python3" "''${ROOT}/tools/prepare-release.py" "$@"
''
