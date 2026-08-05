#!/bin/bash -eu

fuzz_dir="$SRC/nativelink/nativelink-test/fuzz"

cargo fuzz build --fuzz-dir "$fuzz_dir"
cp "$fuzz_dir/target/x86_64-unknown-linux-gnu/release/cas_config" "$OUT/cas_config"
