---
name: nativelink-dependency-update
description: Use when updating NativeLink Cargo dependencies, Bazel module dependencies, rules_rust pins, lock files, toolchains, or generated dependency metadata.
---

# NativeLink Dependency Updates

Use this skill for dependency bumps, Rust toolchain changes, Bazel module changes, lock file updates, and dependency cleanup.

## Files To Inspect

NativeLink keeps Cargo and Bazel dependency state connected. Check the relevant set before editing:

- `Cargo.toml`
- crate-level `Cargo.toml` files
- `Cargo.lock`
- `MODULE.bazel`
- `MODULE.bazel.lock`
- `.bazelrc`
- `flake.nix` and `flake.lock` when tooling comes from Nix

`MODULE.bazel` uses `crate.from_cargo` with workspace crate manifests. Cargo changes can affect Bazel resolution.

## Update Rules

- Prefer the repository's established tooling over hand-editing generated lock content.
- Use `cargo update -p <crate>` for targeted Cargo bumps when appropriate.
- Use Bazel module tooling for Bazel dependency lock updates when required.
- If changing workspace lints or flags, check whether `.bazelrc` must be regenerated. The file notes that lint config is kept in sync with the top-level Cargo config through `tools/generate-bazel-rc`.
- Don't delete or regenerate broad lock files unless the task explicitly requires it.
- Keep feature changes narrow. Feature unification can change behavior across crates.

## Validation

Use a dependency-specific verification ladder:

```bash
cargo check --all
bazel build //:nativelink
```

For Rust dependency bumps:

```bash
cargo test --all --profile=smol
bazel test //...
```

For Bazel, rules, or toolchain changes:

```bash
bazel build //...
bazel test //...
```

When tool versions mismatch locally, enter the Nix development shell:

```bash
nix --extra-experimental-features nix-command --extra-experimental-features flakes develop
```

## Final Report

Include:

- Dependency names and old/new versions.
- Lock files or generated metadata changed.
- Commands run.
- Any known resolver, feature, or platform risk.
