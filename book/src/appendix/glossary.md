# Appendix D: Glossary

## A

**AC (Action Cache)** — A cache that maps action digests to action results. When a client asks "has this action been run before?", the AC provides the answer. See [REAPI from First Principles](../part2/reapi.md).

**Action** — A unit of work: a command, its input tree, and its platform requirements. The fundamental unit of remote execution.

**Action Digest** — The hash of the `Action` proto (which itself contains the command digest, input root digest, and platform). This is the cache key for the AC.

## B

**Blake3** — A hash function alternative to SHA-256. Faster on modern hardware. Supported by NativeLink via the Capabilities service negotiation.

**BEP (Build Event Protocol)** — A streaming protocol for build telemetry. Clients send events (action started, completed, failed) to a BEP server for monitoring and analytics.

**ByteStream** — gRPC service for streaming large blobs. Used when blobs exceed the batch API size limit (~4MB).

## C

**CacheLookupScheduler** — A scheduler *wrapper* (config key `cache_lookup`) that checks the Action Cache before dispatching; on a hit it returns the cached result without involving a worker, and on a miss it delegates to a nested `scheduler`. See [Appendix B: Scheduler Catalog](./scheduler-catalog.md).

**CAS (Content-Addressable Storage)** — Storage where every blob is keyed by its content hash. The fundamental storage primitive in REAPI. See [Why Content-Addressing Works](../part2/content-addressing.md).

**Container Image** — A filesystem tree plus metadata packaged per the OCI Image Spec, addressed by digest and distributed through a registry. In NativeLink a container image plays two roles: it can be the toolchain a worker runs actions inside (named via the `container-image` platform property, passed through as a `priority` property — see [Container-Based Toolchains](../part5/containers.md)).

**Content-Addressing** — A scheme where the name/key of data is derived from its content. Guarantees: same content = same key, same key = same content. See [Mental Model](../part1/mental-model.md).

## D

**Digest** — A `(hash, size_bytes)` pair that uniquely identifies a blob. The fundamental identity type in REAPI.

**DICE** — Deterministic Incremental Computation Engine. Buck2's internal graph evaluation framework. Not part of NativeLink itself, but NativeLink serves as the execution backend for DICE-computed actions.

## E

**Entrypoint** — A command prepended to every action's argv on a worker. Used to wrap actions in containers, apply resource limits, or handle timeouts.

**Eviction Policy** — Rules for removing data when a store is full. Configured with `max_bytes`, `max_count`, `max_seconds`, and `evict_bytes`.

## F

**FastSlowStore** — A composite store with a fast tier (typically memory or local disk) and a slow tier (typically cloud storage). Reads check fast first; writes go to both. See [Store Composition](../part3/store-composition.md).

**FindMissingBlobs** — The most frequent REAPI call. Client sends a list of digests; server responds with which ones are missing. Drives efficient upload (only send what's new).

## G

**gRPC** — The transport protocol for REAPI. NativeLink serves all services via gRPC (with optional HTTP/2 multiplexing and TLS).

## H

**Hermeticity** — The property that an action's outputs depend only on its declared inputs, not on external state. Critical for cache correctness. See [The Execution Contract](../part2/execution-contract.md).

## I

**Instance Name** — An opaque namespace string in REAPI requests. Used to route to different service configurations. Bazel defaults to `""`, Buck2 defaults to `"main"`.

## L

**LRE (Local Remote Execution)** — NativeLink's framework for Nix-based hermetic toolchains that are identical locally and remotely. See [Nix and LRE](../part5/nix-lre.md).

## M

**Merkle Tree** — A hash tree where each node's hash includes its children's hashes. Used to represent directory structures in CAS. Changing one leaf changes all ancestor hashes.

## N

**Namespace Isolation** — Linux kernel feature (`unshare()`) that isolates processes (PID namespace), filesystems (mount namespace), and IPC. Used by workers for action sandboxing.

## P

**Platform Properties** — Key-value string pairs that describe what an action needs (client-side) or what a worker provides (server-side). The bridge between actions and workers. See [Platform Properties](../part4/platform-properties.md).

**PropertyModifierScheduler** — A scheduler *wrapper* (config key `property_modifier`) that rewrites platform properties — `add`, `remove`, or `replace`, applied in order — before forwarding to a nested `scheduler`. See [Appendix B: Scheduler Catalog](./scheduler-catalog.md).

**PropertyType** — NativeLink's classification of platform properties: `minimum` (numeric, >=), `exact` (string equality), `priority` (informational), `ignore` (allowed but unchecked).

## R

**REAPI (Remote Execution API)** — The gRPC protocol for remote caching and execution, defined by the Bazel team. Implemented by NativeLink, Buildbarn, BuildFarm, EngFlow, and BuildBuddy. See [REAPI from First Principles](../part2/reapi.md).

**RefStore** — A store that references another named store by name. Enables sharing a single physical store across multiple service configurations without config duplication.

## S

**Scheduler** — The component that receives Execute RPCs and dispatches actions to matching workers. Maintains the action queue and worker pool.

**SimpleScheduler** — The primary *leaf* scheduler (config key `simple`) that owns the action queue, the connected-worker pool, and the capability index matching actions to workers via `supported_platform_properties`. It terminates any wrapper chain and is the only scheduler with the experimental Redis state backend. See [Appendix B: Scheduler Catalog](./scheduler-catalog.md).

**Store** — NativeLink's fundamental abstraction. Any component that implements `has`/`update`/`get_part` for content-addressed blobs. See [The Store Trait](../part3/store-trait.md).

**StoreDriver** — The implementor trait for stores. Concrete store types implement `StoreDriver`; callers use the type-erased `Store` wrapper.

## T

**Toolchain** — The compiler, linker, standard library, and supporting tools used to build software. Not a concept in REAPI (which only has platform properties), making toolchain management an operational challenge. See [The Toolchain Problem](../part5/toolchain-problem.md).

## W

**Worker** — A NativeLink process that executes actions. Fetches inputs from CAS, runs commands, uploads outputs. Stateless and disposable. See [Workers](../part4/workers.md).

**Worker API** — Internal gRPC API between workers and the scheduler. Not part of REAPI. Handles registration, heartbeat, and result reporting.
