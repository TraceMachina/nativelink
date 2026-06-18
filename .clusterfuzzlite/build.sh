#!/bin/bash -eu

cargo fuzz build --manifest-path "$SRC/nativelink/nativelink-test/fuzz/Cargo.toml"
cp "$SRC/nativelink/nativelink-test/fuzz/target/x86_64-unknown-linux-gnu/release/cas_config" "$OUT/cas_config"
