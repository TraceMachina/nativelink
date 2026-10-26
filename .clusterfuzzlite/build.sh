#!/bin/bash -eu

fuzz_dir="$SRC/nativelink/nativelink-test/fuzz"

cargo fuzz build --fuzz-dir "$fuzz_dir"
mapfile -t targets < <(
    cargo metadata \
        --manifest-path "$fuzz_dir/Cargo.toml" \
        --no-deps \
        --format-version 1 |
        jq -r '.packages[0].targets[] | select(.kind | index("bin")) | .name'
)
for target in "${targets[@]}"; do
    cp "$fuzz_dir/target/x86_64-unknown-linux-gnu/release/$target" "$OUT/$target"
done
