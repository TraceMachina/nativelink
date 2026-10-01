# Part II: NativeLink for Contributors

Part I taught you to *operate* NativeLink — to reason about REAPI, compose stores, deploy a fleet, and chase a cache miss. This part teaches you to *change* it.

That is a different skill, and this codebase does not surrender it by osmosis. It is a genuinely well-built Rust system — a store trait clean enough to compose storage topologies in config instead of code, a scheduling model that holds up at billions of requests a month — but "well-built" and "quick to navigate on day one" are not the same thing. The abstractions are sharp, the indirection is real (type erasure, two-phase wiring, trait objects behind `Pin<&Self>`), and the hardest correctness properties live in seams that are invisible until you've been bitten. A new contributor can spend a week just building a mental map that this part hands you in an afternoon.

This part is that map, in the order you actually need it:

- **[Orientation](./01-orientation.md)** — the crate map and the "where do I go to change X" table. Start here; it turns a wall of `nativelink-*` directories into a system you can navigate.
- **[Building and testing](./02-building-and-testing.md)** — the Bazel/Cargo duality and the specific traps (the pinned nightly rustfmt, the deny-level clippy aspect, the MSRV, the DST fuzzer) that `cargo test` passing will not save you from.
- **[Core abstractions](./03-core-abstractions.md)** — the traits and idioms you will touch in every change: the Store trait from the implementor's side, the scheduler and worker traits, the service layer, and the cross-cutting patterns (the error type, the buf-channel, `background_spawn!`, the config system).
- **[The data path](./04-data-path.md)** — one request traced end to end, with file-and-line waypoints, so you can follow *any* request by analogy. This is the chapter that makes the rest navigable.
- **[How-to recipes](./05-recipes.md)** — concrete, copy-from-the-real-exemplar steps for the common changes: add a store, a scheduler backend, a worker property, a gRPC surface, a config option, a test, a metric.
- **[The hard parts](./06-concurrency-and-correctness.md)** — concurrency, cancellation, and consistency: the failure families, drawn from real bugs, that generate almost every correctness defect in the system. If you read one chapter before touching the scheduler, the worker, or a store, read this one.
- **[Contributing](./07-contributing.md)** — getting a change merged without fighting CI: the verify-before-push checklist, the machine-checked PR template, and the house style.

A note on how this part was built, because you'd work it out anyway: it was written by agents reading the real source directly, and the concurrency chapter in particular is distilled from an adversarial stability pass that found and fixed a drawer-full of real races across the scheduler, the worker, and the stores. The bugs in that chapter are not hypotheticals — they are the actual shapes this system fails in, with the PRs that fixed them. The credit for the engine runs upstream to `TraceMachina/nativelink`; this guide just makes it legible.

The goal is unapologetic: to roughly double the speed at which a competent Rust engineer becomes genuinely dangerous in this codebase. If you finish this part and still can't find where a cache miss turns into a scheduled action, we failed — tell us where it lost you.
