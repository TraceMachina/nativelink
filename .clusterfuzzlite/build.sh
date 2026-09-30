#!/bin/bash -eu

fuzz_dir="$SRC/nativelink/nativelink-test/fuzz"

# Targets are built individually: cas_config has minimal deps and must
# always build; scheduler_race pulls workspace crates whose rust-version
# pin can outrun the oss-fuzz base image's nightly, so it degrades
# gracefully until the image catches up.
cargo fuzz build --fuzz-dir "$fuzz_dir" cas_config
cp "$fuzz_dir/target/x86_64-unknown-linux-gnu/release/cas_config" "$OUT/cas_config"
if cargo fuzz build --fuzz-dir "$fuzz_dir" scheduler_race; then
    cp "$fuzz_dir/target/x86_64-unknown-linux-gnu/release/scheduler_race" "$OUT/scheduler_race"
else
    echo "scheduler_race skipped: toolchain too old for workspace rust-version pin"
fi
