{
  writeShellScriptBin,
  regclient,
}:
writeShellScriptBin "regctl-ghcr-login" ''
  set -xeuo pipefail
  echo $GHCR_PASSWORD | ${regclient}/bin/regctl \
    login ghcr.io \
    --user=$GHCR_USERNAME \
    --pass-stdin
''
