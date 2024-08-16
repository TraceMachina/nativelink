{
  nativelink,
  writeShellScriptBin,
}:
writeShellScriptBin "prevent-store-conflict-test" ''
  set -xuo pipefail

  cat > store_conflict_test.json <<EOF
  {
    "stores": {
      "FILESYSTEM_STORE": {
        "compression": {
          "compression_algorithm": {
            "lz4": {}
          },
          "backend": {
            "filesystem": {
              "content_path": "~/.cache/nativelink/content_path-cas",
              "temp_path": "~/.cache/nativelink/tmp_path-cas",
              "eviction_policy": {
                // 10gb.
                "max_bytes": 10000000000,
              }
            }
          }
        }
      }
    },
    "servers": [{
      "listener": {
        "http": {
          "socket_address": "0.0.0.0:50053"
        }
      },
      "services": {
        "cas": {
          "main": {
            "cas_store": "FILESYSTEM_STORE"
          }
        },
        "ac": {
          "main": {
            "ac_store": "FILESYSTEM_STORE"
          }
        },
        "capabilities": {},
        "bytestream": {
          "cas_stores": {
            "main": "FILESYSTEM_STORE",
          }
        }
      }
    }]
  }
  EOF

  nativelink_output="$(${nativelink}/bin/nativelink ./store_conflict_test.json 2>&1)"

  rm ./store_conflict_test.json

  print_error_output=$(cat <<EOF
  Error: Error { code: InvalidArgument, messages: ["CAS and AC cannot use the same store 'FILESYSTEM_STORE' in the config"] }
  EOF)

  if [ "$nativelink_output" = "$print_error_output" ]; then
    echo "The output of nativelink matches the print_error output."
  else
    echo 'The output of nativelink does not match the print_error output:'
    diff <(echo "$nativelink_output") <(echo "$print_error_output") >&2
    exit 1
  fi
''
