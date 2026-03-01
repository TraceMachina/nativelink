{
  writeShellScriptBin,
  rust,
  ...
}:
writeShellScriptBin "docs" ''
  set -xeuo pipefail

  ${rust}/bin/cargo doc --no-deps \
      -p nativelink-config \
      -p nativelink-error \
      -p nativelink-macro \
      -p nativelink-proto \
      -p nativelink-scheduler \
      -p nativelink-service \
      -p nativelink-store \
      -p nativelink-util \
      -p nativelink-worker \
      --open
''
