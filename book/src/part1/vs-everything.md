# NativeLink vs. Everything Else

The remote execution space has a handful of serious implementations. Here is how they differ where it matters — stated fairly, because a skeptical evaluator reads this before trusting anything else in the book.

## The Landscape

| System | Language | License | Self-host | Managed cloud |
|--------|----------|---------|-----------|---------------|
| **NativeLink** | Rust | `FSL-1.1-Apache-2.0` | Yes | NativeLink Cloud (TraceMachina) |
| **Buildbarn** | Go | `Apache-2.0` | Yes | — |
| **BuildFarm** | Java | `Apache-2.0` | Yes | — |
| **EngFlow** | — (closed) | Commercial | Yes (licensed) | Yes |
| **BuildBuddy** | Go | `Apache-2.0` + enterprise | Yes | Yes |

All five implement both remote caching and remote execution over REAPI; that is the price of admission, not a differentiator. What separates them is the runtime, the operational model, and the license. Every one of them can be self-hosted — including the commercial products, which offer an on-your-own-infrastructure deployment alongside their cloud. That cuts both ways for NativeLink too: TraceMachina runs **NativeLink Cloud** as a managed service, so "self-hosted or managed" is a deployment choice here, not a missing capability.

## Buildbarn

The most direct architectural comparison. Buildbarn is written in Go, is mature, and has been used at scale. The difference that shows up first is storage: Buildbarn has a fixed set of storage backends with fixed composition semantics. NativeLink's store algebra lets you compose arbitrary storage topologies in config without touching code.

Buildbarn's worker model is also more rigid — it splits into separate `bb-worker`, `bb-scheduler`, `bb-storage`, and `bb-runner` binaries. NativeLink is one binary with one config file. This matters less at scale (you deploy different configs per role anyway) but matters enormously for development, testing, and debugging.

Go's garbage collector is a real consideration at the tail. Modern Go targets sub-millisecond stop-the-world pauses, so the horror stories of multi-millisecond p99 spikes are mostly historical — but a collector you tune is still a variable in the latency budget. Rust has no garbage collector, so that variable is absent rather than minimized. Whether the difference is measurable depends on your scale: at billions of `CAS` requests per month it shows up in tail latency; below that it is mostly noise. This is a genuine engineering tradeoff, not a knockout.

## BuildFarm

BuildFarm is one of the oldest open-source Bazel remote-execution systems, long housed in the `bazelbuild` organization. It has real deployment history and it works. The costs are the ones a mature JVM codebase carries: the heaviest deployment story of the group, a larger operational footprint than the Go or Rust options, and a codebase that is the hardest of the five to modify when you need to.

If you already run BuildFarm and it works, the migration cost may not be worth it. If you are starting fresh, the Go and Rust options are lighter to operate.

## EngFlow and BuildBuddy

The commercial options. Both offer a managed cloud service *and* a self-hosted deployment, so "managed versus self-hosted" is a deployment choice with either, not a fork in the road.

**EngFlow** is closed source; you run it under a commercial license, on your own infrastructure or theirs. You get a polished product and vendor support; you do not get to read or patch the server when a failure mode is novel.

**BuildBuddy's** core is open-source Go under `Apache-2.0` and self-hostable, with a proprietary enterprise edition and a managed cloud layered on top. The open core means you can inspect and modify the parts that ship under `Apache-2.0`; the enterprise features are where the closed surface begins.

The tradeoff with either is the usual one: a supported product versus full control over your data, your performance characteristics, and your failure modes. If your organization cannot or will not operate infrastructure, a managed service — NativeLink Cloud included — is a legitimate answer. If you want that control, you operate your own. This book is about operating your own.

## What Actually Matters

The choice between remote execution backends comes down to three questions:

1. **Can it sustain your throughput?** At billions of requests, runtime overhead stops being theoretical. Rust's lack of a garbage collector removes tail-latency variance that a GC'd runtime has to tune for — not a knockout for Go, whose collector is genuinely good now, but one fewer variable in the budget.

2. **Can you compose the storage topology you need?** Tiered storage (fast SSD cache plus slow S3 backend), deduplication, compression, cross-region replication — these are not features you want to patch into a codebase to add. NativeLink's store algebra handles all of them in config.

3. **Can you debug it when it breaks?** A single binary with a single config file and structured logging is easier to reason about than a mesh of five binaries, three config formats, and state scattered across multiple databases.

On all three, NativeLink competes hard — the runtime, the store algebra, and the single-binary model are why it stands up well against the whole field.
