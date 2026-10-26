#!/bin/bash -eu

fuzz_dir="$SRC/nativelink/nativelink-test/fuzz"

cargo fuzz build --fuzz-dir "$fuzz_dir"
for target in cas_config reapi_messages; do
    cp "$fuzz_dir/target/x86_64-unknown-linux-gnu/release/$target" "$OUT/$target"
done
