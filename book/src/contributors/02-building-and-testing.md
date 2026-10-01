# Building and Testing (For Real)

NativeLink has two build systems — Cargo and Bazel — and they do not check the same things. The single most expensive mistake a new contributor makes is running `cargo test`, watching it pass, pushing, and then losing an hour to CI failures that `cargo test` structurally cannot catch: a formatting diff from a *nightly* rustfmt rule, or a clippy deny that only runs inside a Bazel aspect. This chapter is the honest map of what each path verifies, which traps cost people time, and a checklist that catches them before you push.

## The Cargo Path

For day-to-day work, Cargo is faster and more familiar. Build and test one crate at a time:

```bash
cargo build -p nativelink-store
cargo test  -p nativelink-store
cargo test  --all --profile=smol   # what native-cargo CI runs
```

CI's native-cargo lane runs `cargo test --all --profile=smol` (`.github/workflows/native-cargo.yaml:60-61`). The `smol` profile is defined in the root manifest — `opt-level = "z"`, stripped, no debug — and exists purely to keep the `target/` directory from ballooning from ~12GB to ~1GB (`Cargo.toml:19-25`). Use it when disk is tight; use the default `dev` profile when you want fast incremental iteration on one crate.

### The MSRV is enforced by the binary, not the libraries

The workspace pins `rust-version = "1.97.1"` on the **root `nativelink` package** (`Cargo.toml:10-13`), and the same `1.97.1` is the MSRV recorded in `clippy.toml:96` and pinned as the stable toolchain in the flake (`local-remote-execution/overlays/rust-config.nix:2`). The subtlety worth knowing: `rust-version` sits on the root *binary* crate, so it is that crate's build that `cargo` holds to 1.97.1. Per-crate *library* builds — `cargo build -p nativelink-store` — do not themselves carry the pin and will often compile on a newer or (slightly) older rustc. That is convenient for quick iteration, but do not trust it: the only build that matters for merge is the one that includes the binary. Build the whole workspace, or the root crate, before you believe you are MSRV-clean.

### The `nativelink_test` macro

Tests are annotated `#[nativelink_test]`, not `#[tokio::test]`. The macro lives at `nativelink-macro/src/lib.rs:36-99` and expands each test into:

- `#[tokio::test(...)]` — if you pass tokio attributes like `flavor = "multi_thread"`, they are forwarded; otherwise you get tokio's default single-thread, current-thread runtime (`nativelink-macro/src/lib.rs:51-54,75`).
- `#[::tracing_test::traced_test]` — installs a tracing subscriber so the test's spans and logs are captured (`nativelink-macro/src/lib.rs:76`).
- A wrapping `error_span!(<test name>)` and a call to `common::reseed_rng_for_test()`, so the RNG is deterministically seeded per test (`nativelink-macro/src/lib.rs:78-81`).
- A `logs_assert` that *fails the test* if any captured log line looks like non-redacted binary data (`" data: b"`), with a documented exception for one AWS runtime line (`nativelink-macro/src/lib.rs:82-91`).

Two things follow. First, you get tracing and deterministic RNG for free — use them. Second, the macro itself calls `Runtime::block_on` under the hood, which is a disallowed method (see below); it suppresses that lint with a scoped `#[expect(...)]` so you don't have to (`nativelink-macro/src/lib.rs:71-74`). Write `#[nativelink_test]`, not raw `#[tokio::test]`, and you inherit all of this.

## Trap 1: Formatting Uses a Pinned *Nightly* rustfmt

`cargo fmt` with your stable toolchain will pass locally and still produce a CI formatting diff. The reason is in `.rustfmt.toml`:

```toml
imports_granularity = "Module"
group_imports = "StdExternalCrate"
unstable_features = true
```

(`.rustfmt.toml:1-7`.) `imports_granularity` and `group_imports` are **nightly-gated** rustfmt options — stable rustfmt silently ignores them, so stable `cargo fmt` will not reorganize your imports the way CI expects, and CI's formatting check will then reject the diff.

CI formats with a **pinned nightly rustfmt**, resolved through the rust-overlay in the flake. The nightly toolchain version is pinned at `local-remote-execution/overlays/rust-config.nix:3` and the rustfmt pre-commit hook is wired to that exact nightly build (`tools/pre-commit-hooks.nix:100-106`). The CI formatting gate runs via `nix flake check`, which executes those pre-commit hooks (`.github/workflows/pre-commit.yaml`).

To format the way CI does, use the project's own command rather than your ambient toolchain:

```bash
# Canonical, from CONTRIBUTING.md:311 — formats via the Bazel-managed toolchain
bazel run --config=rustfmt @rules_rust//:rustfmt

# Or run the full pre-commit suite (rustfmt + typos + vale on docs), AGENTS.md:200
pre-commit run -a
```

If you must drive rustfmt directly, use the pinned nightly and pass `--edition 2024` — the workspace is edition 2024 (`Cargo.toml:11`), and running rustfmt against the wrong edition changes how it formats (e.g. the import style under edition 2024 differs). Stable `cargo fmt` is not a substitute; treat it as a convenience that does not match the gate.

## Trap 2: Clippy Runs at Deny-Level in a Bazel Aspect — `cargo test` Never Runs It

This is the big one. The native-cargo CI lane runs `cargo test --all --profile=smol` and nothing else — **no clippy** (`.github/workflows/native-cargo.yaml:60-61`). Clippy runs on the *Bazel* side, as an aspect attached to `bazel test`:

```
# .bazelrc:67-68
build --aspects=@rules_rust//rust:defs.bzl%rustfmt_aspect
build --aspects=@rules_rust//rust:defs.bzl%rust_clippy_aspect
```

```
# .bazelrc:186-188 — the aspects only *fail the build* when their output groups are requested, which happens on test
test --output_groups=+rustfmt_checks
test --output_groups=+clippy_checks
```

So `bazel test //...` is what actually enforces clippy, via the `rust_clippy_aspect` from `rules_rust`. "cargo test passes" is **not** "CI passes." The lint set is strict — the root manifest denies `clippy::all`, `clippy::nursery`, and `clippy::pedantic` wholesale (`Cargo.toml:146-148`) — and several specific denies will bite:

- **`disallowed-methods`** (`Cargo.toml:157`) — configured in `clippy.toml:1-16`. **`tokio::spawn` and `tokio::task::spawn` are banned**, as are the runtime builders and `block_on`. Use `nativelink-util`'s wrappers instead: `nativelink_util::task::spawn` or `background_spawn!` for fire-and-forget tasks, `spawn_blocking` for blocking work (`clippy.toml:11,13-14`). This is enforced, not advisory.
- **`doc-markdown`** (`Cargo.toml:158`) — identifiers in doc comments must be in backticks (`` `StoreKey` ``, not `StoreKey`), or clippy fails. The allow-list of bare identifiers lives in `clippy.toml:18-95` (things like `GitHub`, `macOS`); anything not on it needs backticks.
- **`single-match-else`** (`Cargo.toml:177`), **`items-after-statements`** (line 165), **`redundant-closure-for-method-calls`** (line 173), **`semicolon-if-nothing-returned`** (line 174), **`use-debug`** (line 182), and a long list of other restriction denies — all fail the build at deny-level.

Run clippy locally before you push. The fast approximation is:

```bash
cargo clippy --all-targets -- -D warnings
```

This runs the workspace lint config from `Cargo.toml` and catches the vast majority — the `disallowed_methods` and `doc-markdown` denies included, since those come from `clippy.toml` + the manifest lints that Cargo also reads. The authoritative gate is still `bazel test //...` (it runs the aspect in the exact CI configuration), but cargo-clippy is the quick feedback loop that stops you from pushing an obvious `tokio::spawn`.

## The Bazel Path

Bazel is the source of truth for CI. The commands (`AGENTS.md:118-121`):

```bash
bazel test //...                                   # everything; first run 10-20 min
bazel test //nativelink-store/tests:s3_store_test  # one target
bazel build //nativelink:nativelink                # the server binary
```

`bazel test //...` builds every target, runs every test, **and** runs the rustfmt and clippy aspects — it is the closest local reproduction of what CI checks.

### LRE and the RBE toolchain matrix

**LRE** (Local Remote Execution) lets you run Bazel builds against a NativeLink remote executor on your own machine, using hermetic toolchains so the remote and local environments match. The config is generated into `lre.bazelrc` from `local-remote-execution/flake-module.nix`, and selects remote execution with flags like `build:lre --define=EXECUTOR=remote` and the generated CC toolchain/platform under `@local-remote-execution//generated-cc/config` (see `local-remote-execution/README.md`). This is the dogfooding path — NativeLink building NativeLink through NativeLink.

The CI remote lanes live in `ci.bazelrc`. The `nl-rbe` config is the remote-execution matrix: it pins the execution platform and host platform to the hermetic LLVM glibc 2.28 platform, registers all rust toolchains, and tunes remote retries and eviction behavior (`ci.bazelrc:51-65`). A companion `nl-cache` config drives the remote-cache-only lanes (`ci.bazelrc:25-44`). The nightly channel for the aspects is selected by `build:nightly` (`.bazelrc:195`). You rarely invoke these by hand; know they exist so a red CI lane named `nl-rbe` or `nl-cache` tells you *where* the failure is.

## The DST Fuzzer

`nativelink-test/fuzz/` is a standalone `cargo-fuzz` package (`nativelink-fuzz`), excluded from the workspace (`Cargo.toml:3-7`). It defines two targets (`nativelink-test/fuzz/Cargo.toml:30-43`):

- **`cas_config`** — feeds arbitrary bytes to the `CasConfig` JSON5 parser, hunting for parser panics.
- **`scheduler_race`** — the deterministic simulation ("DST") fuzzer for the scheduler. It drives concurrent scheduler operations under a controlled, replayable schedule to shake out race conditions, calling into `nativelink_fuzz::scheduler_race::run(data)` (`nativelink-test/fuzz/fuzz_targets/scheduler_race.rs`).

Run a smoke campaign with `cargo-fuzz` installed:

```bash
cargo install cargo-fuzz   # once
cd nativelink-test/fuzz
cargo fuzz run scheduler_race -- -max_total_time=60   # 60s smoke
cargo fuzz run cas_config    -- -max_total_time=60
```

A short campaign is a cheap pre-push sanity check if you touched the scheduler or the config parser; the fuzzer is a corpus-driven search, so longer runs find deeper bugs. The *why* behind `scheduler_race` — determinism as a tool for reproducing concurrency bugs — belongs to the concurrency chapter; this is the operational "how to run it."

## Trap 3: The PR-Template CI Gate

CI rejects a pull request whose description is missing four verbatim headings. They are enforced by `.github/workflows/pr-template-check.yaml`, which runs `.github/scripts/check_pr_body.py` against your PR body. The required headings, exactly as they appear in `.github/pull_request_template.md`, are:

```
## What and why
## How was this verified?
## Risk
## AI assistance
```

(`.github/pull_request_template.md:10,17,27,38`.) The check also enforces minimum content lengths — roughly 40 characters each for "What and why", "How was this verified?", and "Risk", and a non-empty "AI assistance" — so an empty heading does not satisfy it (`.github/scripts/check_pr_body.py`). The `AI assistance` section is a disclosure requirement; `CONTRIBUTING.md:18-33` explains the policy. Start from the template (GitHub pre-fills it), fill each section with real content, and keep the four headings verbatim.

## The "Verify a Change Before You Push" Checklist

Copy-pasteable. Run from the repo root. Build upward from the crate you changed.

```bash
# 1. Build and unit-test the crate(s) you touched, plus everything above them.
cargo test -p <crate-you-changed>
cargo build -p nativelink            # the root bin — confirms MSRV 1.97.1 and that the stack still links

# 2. Format the way CI does — pinned NIGHTLY rustfmt, not stable `cargo fmt`.
bazel run --config=rustfmt @rules_rust//:rustfmt
#   (or:) pre-commit run -a

# 3. Clippy at deny-level — cargo test does NOT run this.
cargo clippy --all-targets -- -D warnings
#   watch for: tokio::spawn (use nativelink_util::task::spawn / background_spawn!),
#   bare identifiers in doc comments (backtick them), single-match-else, use-debug.

# 4. The authoritative gate: Bazel test runs the fmt + clippy aspects in CI config.
bazel test //...                     # first run 10-20 min; this is what CI actually checks

# 5. If you touched the scheduler or the config parser, run a fuzz smoke campaign.
(cd nativelink-test/fuzz && cargo fuzz run scheduler_race -- -max_total_time=60)

# 6. Write the PR body with the four verbatim headings or CI will reject it:
#      ## What and why   ## How was this verified?   ## Risk   ## AI assistance
```

If you only have time for two of these, make them step 2 (nightly fmt) and step 3 (clippy): they are the two gates `cargo test` cannot see, and they are where new contributors lose the most time.

**Source for this chapter:** [`Cargo.toml`](https://github.com/TraceMachina/nativelink/blob/main/Cargo.toml), [`clippy.toml`](https://github.com/TraceMachina/nativelink/blob/main/clippy.toml), [`.rustfmt.toml`](https://github.com/TraceMachina/nativelink/blob/main/.rustfmt.toml), [`.bazelrc`](https://github.com/TraceMachina/nativelink/blob/main/.bazelrc), [`.github/workflows/`](https://github.com/TraceMachina/nativelink/tree/main/.github/workflows), [`nativelink-macro/src/lib.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-macro/src/lib.rs), and the fuzz package under `nativelink-test/fuzz`.
