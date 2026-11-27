#!/bin/bash -eu

fuzz_dir="$SRC/nativelink/nativelink-test/fuzz"
target="${CFL_EXTRA_FUZZ_TARGET:-${FUZZ_TARGET:-}}"

if [[ -n $target ]]; then
    cargo fuzz build --fuzz-dir "$fuzz_dir" "$target"
else
    cargo fuzz build --fuzz-dir "$fuzz_dir"
fi
mapfile -t targets < <(
    cargo metadata \
        --manifest-path "$fuzz_dir/Cargo.toml" \
        --no-deps \
        --format-version 1 |
        jq -r '.packages[0].targets[] | select(.kind | index("bin")) | .name'
)
if [[ -n $target ]]; then
    targets=("$target")
fi
for target in "${targets[@]}"; do
    cp "$fuzz_dir/target/x86_64-unknown-linux-gnu/release/$target" "$OUT/$target"
done
