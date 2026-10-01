# Introduction

Content addressing is the best idea in build infrastructure, and almost nobody takes it all the way.

The pitch is one sentence: name everything by the hash of its bytes, and identity stops being a guess. A source file, a compiler output, an action result — if the hash matches, the data is correct, and no clock, path, tag, or promise gets a vote. Build systems have understood this for a decade. What they keep missing is how far it goes. Your source tree is content. Your build outputs are content. Your *toolchain* is content. The thing your registry calls an "image" is content wearing a costume. Push the idea to its conclusion and one truth falls out: there should be one content-addressed store under all of it, and everything else — execution, toolchain distribution, caching — should be a protocol that speaks to that store.

NativeLink is a serious implementation of that idea. It gives you a Content-Addressable Storage engine and a Remote Execution service in front of it, built so that the `CAS` is not a build-cache implementation detail but the ground truth it already is. This book is the map of that idea, and the honest field guide to the system that implements it.

## Standing on Their Shoulders

*NativeLink is TraceMachina's work, and it is superb. They wrote the engine, in Rust, at a level of care that shows on every hot path: a store trait clean enough to compose storage topologies in config instead of code, a scheduling model that holds up at billions of requests a month, and a discipline about correctness that runs through the whole codebase. They designed it well and then gave it away under an open license, which is the rarer and more generous act. Everything load-bearing in this book is theirs. This guide exists to help you understand their system — most of it is exactly that. When you read a claim in here about speed, store composition, or protocol fidelity, the credit runs upstream to [`TraceMachina/nativelink`](https://github.com/TraceMachina/nativelink).*

## Who This Is For

You build software. You've heard of remote caching or remote execution — maybe you've fought with it. You use Bazel, Buck2, or another build system that speaks REAPI, and you want to understand what actually happens when your build talks to a remote backend, without cargo-culting YAML from a getting-started guide.

If you're evaluating NativeLink against Buildbarn, EngFlow, or BuildFarm, this book makes the architectural differences plain. If you're trying to drag your cache hit rate above 90%, the toolchain chapters will save you weeks.

And if you want to go deeper than operating NativeLink — to read its source, extend it, or contribute upstream — Part II is written for you.

## How This Book Is Organized

This book is in two parts.

**Part I: For Operators** gives you the mental model and the protocol, then the internals you need to run NativeLink well. Chapters cover the 30-second model and how NativeLink compares to alternatives; REAPI from first principles and where it breaks down; the store algebra and how backends compose into arbitrary topologies through a single trait; scheduling, platform properties, and workers; toolchains, which is where most teams get stuck; client integration for Bazel and Buck2; deployment from a single binary on localhost to a production Kubernetes fleet with observability; and end-to-end worked examples.

**Part II: For Contributors** is the developer's half of the book. It orients you in the codebase, walks the crate layout, covers building and testing, and then goes deep on the extension points: writing a new store, the scheduler internals, and the services-and-protocols surface — closing with how to get a change upstream.

## What You'll Learn

By the end of Part I you will understand:

- **REAPI** — the protocol Bazel, Buck2, and NativeLink all speak, from first principles
- **The store algebra** — how NativeLink composes storage backends into arbitrary topologies through a single trait
- **Scheduling and dispatch** — how actions match to workers, and why platform properties are the entire game
- **Toolchains** — the three approaches (Nix/LRE, containers, hermetic cross-compilers) and when each one wins
- **Client integration** — Bazel and Buck2 as equal first-class citizens, with real configs and real gotchas
- **Deployment** — from a single binary on localhost to a production Kubernetes fleet with observability

## How to Read This

Read Part I front to back the first time. Part I and II give you the mental model and the protocol; the stores and scheduling chapters are the internals; toolchains is where most teams get stuck; then client integration, deployment, and worked examples. The code links point into the actual NativeLink source — follow them. If you intend to contribute, Part II picks up where Part I leaves off.

## A Note on Tone

This book is opinionated, because remote execution is too important and too poorly understood for hedging. When something is wrong, we say so. When there's a better way, we show it. When the protocol has gaps, we name them.

The build-system world has spent a decade telling people to "just set `--remote_cache`" and wondering why cache hit rates are terrible. The answer is always toolchains. This book will show you why.

Let's go.
