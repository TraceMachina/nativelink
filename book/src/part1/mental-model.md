# Mental Model

Remote execution rests on exactly one idea: **content-addressing**.

Everything else — the caching, the distribution, the deduplication, the integrity checking — is a consequence of this one idea. If you understand content-addressing, you understand the system. If you don't, no amount of configuration documentation will help you.

## Content-Addressing

A content-addressed store maps `hash(content) → content`. You don't choose where things go. You don't name them. You compute their identity from their bits.

This gives you three properties for free:

1. **Deduplication.** If two actions produce byte-identical outputs, they have the same hash. You store it once.

2. **Integrity.** If you retrieve content by hash and the hash matches, the content is correct. No checksums, no version numbers, no "did something change underneath me."

3. **Cacheability.** If you can deterministically compute the hash of an action's inputs, and the action is deterministic, then the hash of the action *is* the cache key. Cache lookup is a hash table lookup.

## The Action Cache Equation

The Action Cache is conceptually:

```
AC[hash(Command, InputRootDigest, Platform)] = ActionResult {
    output_files: [(path, digest)],
    output_directories: [(path, tree_digest)],
    stdout_digest,
    stderr_digest,
    exit_code,
}
```

The inputs to the hash are:
- **Command**: the argv, environment variables, output paths, working directory
- **InputRootDigest**: the Merkle tree hash of the entire input filesystem
- **Platform**: the execution platform requirements (OS, architecture, toolchain)[^platform]

If any of these change, the hash changes, and you get a cache miss. This is why toolchain management is the entire game — if your toolchain isn't hermetically captured in the platform specification, your action hashes will differ between machines, and your cache is worthless.

## The Merkle Tree

CAS stores files. But actions don't operate on individual files — they operate on directory trees. REAPI represents directory trees as Merkle trees: each directory node contains the digests of its children (files and subdirectories). The root digest identifies the entire tree.

```
RootDigest: abc123
├── src/
│   ├── main.c  → digest: def456
│   └── util.c  → digest: 789abc
├── include/
│   └── util.h  → digest: fed321
└── Makefile    → digest: 112233
```

Changing one file changes its digest, which changes its parent directory's digest, which propagates up to the root. The root digest is a single hash that uniquely identifies the entire input tree. This is how "did my inputs change?" becomes a single comparison.

## Why This Matters for NativeLink

NativeLink's entire architecture follows from content-addressing:

- **Stores** are content-addressed — the key is always a digest (hash + size). This is why they compose: any store that maps `digest → bytes` is interchangeable with any other. That interface is the `StoreDriver` trait — `update` writes the bytes for a key and `get_part` reads them back (`nativelink-util/src/store_trait.rs:621`).

- **The Action Cache** is a content-addressed map from action digests to results. It doesn't need invalidation logic — if the inputs change, the key changes, and there's nothing to invalidate.

- **Workers are stateless.** They fetch inputs by digest, run a command, upload outputs by digest. They hold no state between actions. This is why they can be cattle, not pets — scale them up, kill them, replace them.

- **The scheduler keeps no durable state.** It matches actions to workers by platform properties, holding its action queue and worker registrations in memory (the process wires the scheduler maps up at `src/bin/nativelink.rs:304-317`; `SimpleScheduler` owns an in-memory worker pool and awaited-action queue, `nativelink-scheduler/src/simple_scheduler.rs:124`). It doesn't need to remember what happened before: that state lives only for the lifetime of the process, and failed actions are re-dispatched.

- **Anything with a stable hash becomes an ordinary CAS input.** Because identity is computed from bytes, an OCI toolchain image or a Nix store closure can be addressed by content and dropped into the same store as any build artifact.

Content-addressing turns distributed systems problems into hash table problems. That's the insight. That's the whole thing.

[^platform]: The equation treats `Platform` as a separate hash input for clarity, but in REAPI it is a nested message, not a sibling of `Command`. It appears as `Command.platform` (field 5, `nativelink-proto/build/bazel/remote/execution/v2/remote_execution.proto:686`), which v2.2 deprecated in favor of setting the same properties directly on the action as `Action.platform` (field 10, same file, `:544`). Either way its bytes fold into the action digest, so the equation holds.
