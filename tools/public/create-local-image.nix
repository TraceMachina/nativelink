{writeShellScriptBin}:
writeShellScriptBin "create-local-image" ''
  set -euo pipefail

  nix run .#image.copyTo docker-daemon:local-nativelink:latest
  echo "Created local-nativelink image"
''
