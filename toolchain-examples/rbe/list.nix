{
  writeShellScriptBin,
  jo,
  rbe-toolchain-cmd,
}:
writeShellScriptBin "rbe-toolchain-list" ''
  set -uo pipefail

  ${rbe-toolchain-cmd}

  ${jo}/bin/jo -a "''${!COMMANDS[@]}"
''
