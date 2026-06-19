#!/bin/bash -eu

cd "$SRC/nativelink/nativelink-test/fuzz"
cargo fuzz build
cp target/x86_64-unknown-linux-gnu/release/cas_config "$OUT/cas_config"
