# Contributing: Getting a Change Merged

You have a change. It compiles, your test passes locally, and you're ready to open a PR. This chapter is the difference between that PR going green on the first push and you spending an afternoon chasing red CI on things that have nothing to do with your actual change. Everything here is a lesson paid for in failed CI rounds.

The short version: **CI runs more than you do, denies more than you expect, and parses your PR description.** Here is the full list, in the order it bites.

## The verify-before-push checklist

Run all of this *before* you push, not after CI tells you. It is the exact sequence that would have saved every red round in the stability pass.

1. **`cargo test -p <your-crate>`** — the first reach. Necessary, not sufficient (see below).
2. **`cargo clippy -p <your-crate> --all-targets -- -D warnings`** — *this is the one people forget.* CI runs clippy at deny-level through a Bazel aspect that `cargo test` never invokes. A change that tests clean can still red CI on a lint. The denies that actually bite:
   - **`disallowed_methods`** — `tokio::spawn` is banned; use the repo's `background_spawn!` / `spawn!` macro (it carries tracing context and naming). `std::time::SystemTime::now` and friends are often restricted too. Check `clippy.toml` for the live list.
   - **`doc-markdown`** — an identifier in a doc comment needs backticks. `/// returns StartExecute` fails; `` /// returns `StartExecute` `` passes. This one cascades: clippy runs *per Bazel target*, so the src lint and the test-file lint surface on separate runs — fix one, the next appears.
   - **`single-match-else`** — a `match` with one real arm plus a catch-all should be `if let … else` / `let … else`.
3. **Format with the pinned nightly rustfmt.** CI's formatter is a *nightly* rustfmt pinned by date through the rust-overlay flake — **not** your stable `cargo fmt`. Stable rustfmt silently skips the nightly-gated rules the repo enables (`imports_granularity`, `group_imports`, `StdExternalCrate` import ordering), so `cargo fmt` can report "clean" on a file CI will reject. Materialize the pinned nightly rustfmt from the flake and run it with `--edition 2024` on every file you touched. (Find the exact pin in the flake; it is a dated nightly, and "the wrong nightly" formats differently.)
4. **If you touched a store, the scheduler, or the worker: run the DST fuzzer smoke** (`nativelink-test/fuzz`) and make sure your regression **fails without your fix** — in the ordering that actually triggers the bug (see the concurrency chapter; a green test on the wrong interleaving is a trap).
5. **Rebase on current `main` and re-run.** Upstream moves fast and changes signatures; a clean textual rebase does not mean your code still compiles against the new base. (In the stability pass, a one-commit-behind rebase silently broke four things — a new field on a struct literal, a tuple that gained an element, a bounded vs unbounded channel, and a changed observable in a test — none of which the rebase flagged.)

If all five are green on the metal, CI will almost certainly be green too.

## The PR description is machine-checked

A CI gate (`pr-template-check`) greps your PR body for **four verbatim section headings** and fails the `check` job if any is missing or too short. They are:

- `## What and why` (≥ ~40 chars) — what the change does and the reason it's needed.
- `## How was this verified?` (≥ ~40 chars) — **and this must say how you know it works**, including, for a bug fix, how you know the test fails *without* the change. "Ran the tests" is not an answer; "reverted the guard, the new test fails at line N with `the expected assertion`, restored, it passes" is.
- `## Risk` (≥ ~40 chars) — what breaks if this is wrong, and the blast radius.
- `## AI assistance` (≥ 4 chars) — which AI tools helped, if any. "None" is a complete and honest answer. This exists because of the project's CONTRIBUTING policy on AI-assisted contributions; disclose plainly.

The gate re-runs the instant you *edit* the description, so you get fast feedback — but it also means an older PR whose body predates the gate will red the moment you touch it. Keep the four headings verbatim.

## The rest of the mechanics

- **CLA.** The repo requires a signed CLA (`license/cla` check). Sign it once.
- **One bug per PR.** The stability pass shipped each fix as a parsimonious, single-bug PR with its own regression. That is the house style and it keeps review burden low — a reviewer can hold one interleaving in their head, not six. Resist bundling.
- **Commit messages / changelog.** The repo uses conventional-commit-style prefixes and generates a changelog (`cliff.toml`); check `CONTRIBUTING.md` for the exact convention.
- **The full CI matrix.** Expect: `cargo` dev builds, `bazel` builds, asan/sanitizers, a Windows build, the `rbe *` LRE toolchain matrix, coverage, a Redis store tester, `vale`/docs lints, `typos`, `taplo` (TOML formatting — yes, your `Cargo.toml` edits get format-checked), and the `pr-template-check`. Most are fast; the long poles are asan, the Windows build, and the `rbe *` matrix.
- **Drafts are your friend.** Open as a draft while CI chews; flip to ready once it's green and you've self-reviewed. Nobody gets paged for a draft.

## Where to look when you're stuck

- `AGENTS.md` and `CONTRIBUTING.md` at the repo root — the canonical, maintained process docs. If this chapter and those disagree, those win (and please fix this chapter).
- `llms.txt` / `llms-full.txt` — machine-oriented orientation, useful to humans too.
- The `.github/workflows/` directory — the ground truth for *exactly* what CI runs; when a check reds and you don't recognize it, read its workflow.
- `clippy.toml`, the flake (`flake.nix` / `flake-module.nix`), `rust-toolchain*`, and `Cargo.toml` — the ground truth for the lint set, the rustfmt pin, and the MSRV.

## The mindset

The bar here is high and it is worth meeting: a fix without a fails-without regression is a hope, not a fix; a "verified" claim without the fail→pass evidence is a liability; and a green `cargo test` is the *beginning* of verification, not the end. Match that bar and review goes fast. The project's maintainers are careful people working on a system that has to be correct under load — contribute like you share that constraint, because once it's merged, you do.
