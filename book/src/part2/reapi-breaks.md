# Where REAPI Breaks Down

REAPI is a good protocol. It is not a complete protocol. Understanding where it breaks down is essential for operating a remote execution system correctly — because you'll need to fill the gaps yourself.

## Gap 1: Platform Properties Are Untyped

Platform properties are `repeated Property { string name, string value }`. That's it. No schema, no type system, no standard keys. The protocol says nothing about what `cpu_count`, `OSFamily`, or `container-image` mean. It's convention all the way down.

This creates real problems:

- A client sets `container-image: docker://my-toolchain:latest`. Does the server interpret this? Pull the image? Use it as an opaque match key? The protocol doesn't say.
- A worker advertises `cpu_count: 8`. A client requests `cpu_count: 4`. Is the worker eligible? Is `cpu_count` a minimum, an exact match, or informational? The protocol doesn't say.

The `container-image` question is worth pinning down now. As a platform-property value, `container-image: docker://…` is an *opaque match key*: the scheduler compares it as a string; the server never parses or pulls it. Whether a container is pulled at all is up to the worker, not the protocol (Part V, [Container-Based Toolchains](../part5/containers.md)). The protocol itself has no notion of resolving that URI into content — "does the server pull the image?" is not a question REAPI answers.

NativeLink resolves the untyped-property side with typed property matching in the scheduler config:

```json5
// From nativelink-config/src/schedulers.rs
supported_platform_properties: {
  cpu_count: "minimum",        // worker must have >= requested
  OSFamily: "priority",        // informational, doesn't restrict
  "container-image": "priority", // passed through, not matched
  ISA: "exact",                // must match exactly
}
```

See [`nativelink-config/src/schedulers.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/src/schedulers.rs) for the `PropertyType` enum: `Minimum`, `Exact`, `Priority`, `Ignore`.

But this is NativeLink's interpretation. Buildbarn does something different. EngFlow does something different. No interoperability guarantee covers platform semantics.

## Gap 2: No Toolchain Identity

This is the big one. REAPI has no concept of "toolchain." It has platform properties, which are strings, and it has the `Command` proto, which contains argv. The actual compiler, linker, and libraries used by a worker are completely outside the protocol.

The consequence: two workers with identical platform properties but different toolchain versions will both accept the same action. The first one to complete determines the cached result. If they produce different outputs (because different compiler versions), the cache now contains a result that's correct for one toolchain and wrong for the other.

The protocol cannot detect or prevent this. The only solutions are:

1. **Pin the toolchain in the platform properties** (e.g., `container-image: docker://toolchain@sha256:...`) — an opaque match key, not a fetch (see the note in Gap 1)
2. **Pin the toolchain in the action inputs** (include the compiler binary in the input tree)
3. **Use Nix** (where the toolchain's store path is content-addressed by construction)

Part V of this book is entirely about these approaches.

## Gap 3: No Cache Invalidation

No `InvalidateActionResult` RPC exists. `UpdateActionResult` can overwrite an entry (last-writer-wins), but a client that gets a cache hit never re-executes the action to produce a fresh result — so in practice a stale result stays until evicted by LRU/TTL or the entire cache is cleared.

In practice, you need invalidation when:
- A toolchain bug produced incorrect outputs
- A non-hermetic action leaked host state into the cache
- A worker was compromised

Your options:
- **Clear the entire AC.** Crude but effective. (NativeLink: restart with an empty AC store.)
- **Change the action hash.** Modify the command's environment or platform properties to force a new digest. Bazel's `--host_platform_remote_properties_override` can inject a "cache salt."
- **TTL-based eviction.** Configure short TTLs on the AC store so stale entries expire naturally. Trades cache hit rate for staleness risk.

None of these are great. Cache invalidation is a hard problem even with content-addressing, because the issue is not "is this content correct?" but "was this content produced by a correct process?" Content-addressing guarantees integrity, not provenance.

## Gap 4: No Streaming Progress

The `Execute` RPC returns a stream of `Operation` protos. But the `Operation` message only has states (QUEUED, EXECUTING, COMPLETED) and opaque metadata. There's no standardized way to stream build output (stdout/stderr) during execution.

This means interactive builds — where you want to see compiler warnings as they happen — require out-of-band mechanisms. NativeLink's experimental origin events and BEP integration provide some of this, but it's not part of REAPI proper.

## Gap 5: No Multi-Action Transactions

Each `Execute` call is independent. There's no mechanism for "execute A, then if A succeeds, execute B using A's outputs." Build systems handle this client-side by serializing dependent actions. But this means:

- The client must stay connected for the entire build graph. If it disconnects, partial state is lost.
- Network round-trips between every dependent action pair (client waits for A, downloads result, uploads B's inputs, submits B).
- No server-side optimization of action chains.

Some systems (including NativeLink's `CacheLookupScheduler`) work around this by checking the AC before dispatching, but true server-side graph execution is not in the protocol.

## Gap 6: Output Paths Must Be Declared

The client must declare `output_files` and `output_directories` in the `Command` proto. The worker only uploads these declared paths. Anything else the action writes is lost.

This breaks tools that produce discovery-based outputs (e.g., compilers that generate `.d` dependency files alongside `.o` files). The client must know all output paths before execution. Build systems handle this by hard-coding known patterns or using two-phase builds (run once to discover outputs, run again to capture them).

REAPI v2.1+ added `output_paths` which unifies files and directories, but the fundamental problem remains: you must declare outputs statically.

## Living With the Gaps

These gaps don't make REAPI unusable — they make it incomplete. The protocol handles the common case well. The edge cases require system-level solutions:

- Toolchain identity → solved by deployment architecture (Part V)
- Cache invalidation → solved by operational procedures (clear AC + rebuild)
- Streaming → solved by supplementary protocols (BEP, origin events)
- Transactions → solved by the client (build system dependency graph)
- Output discovery → solved by conventions (known output patterns per language)

NativeLink implements the protocol faithfully and fills many of these gaps with configuration options. But understanding the gaps is essential for operating any REAPI system correctly.
