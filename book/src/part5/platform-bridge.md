# Platform Properties as the Bridge

Platform properties are the only link between client intent and worker capability. They are the bridge between "what the action needs" and "what the worker has." Every toolchain approach — Nix, containers, hermetic downloads — ultimately expresses its identity through platform properties.

Understanding how to use them correctly is the difference between a working cache and a broken one.

## The Chain of Trust

```
Toolchain Identity
       │
       ▼
Platform Property Value (string)
       │
       ▼
Action Hash (includes Platform proto)
       │
       ▼
Cache Key
```

If two actions have the same cache key, the protocol assumes they're identical and the cache can serve either result for the other. If the toolchain identity isn't captured in the platform property, two different toolchains produce the same cache key, and the cache is unsound.

## Property Design Patterns

### Pattern 1: Image Digest (Containers)

```json5
// Scheduler config
supported_platform_properties: {
  "container-image": "priority"  // value passed to the worker, not compared
}
```

Client sets:
```
container-image = docker://registry.example.com/toolchain@sha256:a1b2c3...
```

**Why it works:** The digest is content-addressed. Same digest = same image = same toolchain. Different digest = different action hash = cache miss.

**Why priority, not exact:** `priority` hands the value to the worker without comparing it, so any container worker can run any image. But `priority` is not "match anything" — the scheduler still requires the worker to *advertise* the `container-image` key before it is eligible (`platform_properties.rs:70-75,116-117`; `worker_capability_index.rs:184-193`). This is exactly why LRE worker configs publish `container-image` with an empty value: they advertise the key so `priority`-typed actions match, without pinning a value (`integration_tests/buildstream/buildstream_cas.json5:102`). If you want a property that never restricts matching — eligible even on workers that do not advertise the key at all — that is `ignore`, not `priority` (see [The Matching Algorithm](#the-matching-algorithm-in-detail)).

### Pattern 2: Nix-Derived Image Tag (LRE)

Local Remote Execution does *not* ship a Nix store path as a platform property value. It folds the Nix closure hash into the `container-image` **tag** and reuses Pattern 1. The generated C/C++ platform:

```python
# local-remote-execution/generated-cc/config/BUILD (generated)
exec_properties = {
    "container-image": "docker://lre-cc:zms5771rx1yqb4wd6qbj5f9sb2paq75k",
    "OSFamily": "Linux",
}
```

The Rust toolchain does the same against a published worker image:

```python
# local-remote-execution/rust/platforms/BUILD.bazel (generated)
exec_properties = {
    # tag is from nix eval .#packages.x86_64-linux.nativelink-worker-lre-rs.imageTag
    "container-image": "ghcr.io/tracemachina/nativelink-worker-lre-rs:1hvvyzz0z4rs6d9arlnxc05pxz8qz7zj",
}
```

**Why it works:** the tag *is* the Nix closure hash (`imageTag`). Change any toolchain derivation and the closure hash changes, the tag changes, the `container-image` value changes, and the action hash changes. Identity is structural, not conventional — the same guarantee as Pattern 1, sourced from a Nix closure instead of a registry digest. See [Nix and LRE](./nix-lre.md) for how the tag is generated.

**What `lre-cc` / `lre-rs` actually are:** they are Nix package and worker-image names, and — separately — optional scheduler *pool markers*. A scheduler may declare `"lre-rs": "priority"` and workers advertise the key with an empty value to route Rust actions to Rust-capable workers (`deployment-examples/docker-compose/scheduler-multi-worker.json5:40`; `integration_tests/buildstream/buildstream_cas.json5:59,107`). They carry **no** toolchain identity; the toolchain digest lives in `container-image`. There is no `lre-cc = /nix/store/…clang` platform property anywhere in the codebase.

### Pattern 3: Version String (Hermetic Downloads)

```json5
// Scheduler config
supported_platform_properties: {
  "toolchain-version": "exact"
}
```

Client sets:
```
toolchain-version = zig-0.11.0-clang-17
```

**Why it works (partially):** If you change the version string when you change the toolchain, action hashes change. But it's correctness-by-convention — nothing enforces that the string actually reflects the toolchain content. A typo or stale value breaks correctness silently.

### Pattern 4: Platform Pool (Multi-Tenant)

```json5
// Scheduler config
supported_platform_properties: {
  "pool": "exact",
  cpu_count: "minimum"
}
```

Client sets:
```
pool = ci-linux-x86
cpu_count = 4
```

**Why it works:** Actions land on workers in the specified pool. Pools are configured identically. But identity is still by convention — there's no structural guarantee that all workers in a pool have the same toolchain.

## Composing Properties

Properties compose additively. An action can set multiple properties that together define the execution environment:

```
container-image = docker://toolchain@sha256:abc123  # toolchain identity
cpu_count = 8                                        # resource requirement
OSFamily = Linux                                     # OS requirement
ISA = x86-64                                         # architecture requirement
pool = fast-compile                                  # worker pool routing
```

The action hash includes all of these. Change any one, and you get a different cache key.

## The PropertyModifierScheduler

Sometimes you need to normalize, add, or strip properties before matching. The `PropertyModifierScheduler` wraps another scheduler and transforms properties:

**Source:** [`nativelink-scheduler/src/property_modifier_scheduler.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/property_modifier_scheduler.rs)

```json5
schedulers: [{
  name: "MAIN",
  property_modifier: {
    modifications: [
      // Add a default ISA if the client doesn't specify one.
      { add: { name: "ISA", value: "x86-64" } },
      // Remove internal-only metadata.
      { remove: "internal-trace-id" },
      // Rename a client-facing key to the key workers advertise.
      // `new_name` is required; the value carries over unchanged.
      { replace: { name: "image", new_name: "container-image" } },
      // Value-gated rewrite: only when the old value matches `value`,
      // rename (here to the same key) and substitute `new_value`.
      { replace: { name: "container-image",
                   value: "docker://toolchain:legacy",
                   new_name: "container-image",
                   new_value: "docker://toolchain@sha256:pinned" } }
    ],
    scheduler: { simple: { /* ... */ } }
  }
}]
```

Read the `replace` fields carefully — they are not what the names suggest (`schedulers.rs:243-257`):

- `name` — the property to look for. It is **removed** from the set first.
- `value` — a *match filter*, not the replacement. If set, the rewrite fires only when the existing value equals it; otherwise the property is put back untouched. If omitted, any value matches.
- `new_name` — **required.** The key to insert. To keep the same key, repeat it here.
- `new_value` — optional. The value to insert; if omitted, the existing value carries over.

So `replace` is a *rename-and-optionally-rewrite* operation gated on an optional value match (`property_modifier_scheduler.rs:111-131`). A bare `{ replace: { name, value } }` — a replacement without `new_name` — does not parse: config load rejects it with a missing-field error naming `new_name`.

Use cases:
- **Default injection:** Clients that don't set `ISA` get it added automatically.
- **Key renaming:** Map a client-facing property name onto the key the workers actually advertise.
- **Property stripping:** Remove properties that are meaningful for caching but not for worker matching.

One caution the names invite: modifications run in the scheduler, *after* the client has already hashed the `Platform` proto into the action. They change **which worker matches**, never the cache key. Do not use `replace` to "resolve" a mutable tag to a pinned digest expecting cache safety — the action hash still contains the pre-modification value, so two different pinned digests behind one tag collide on the same cache key. Content-address the value on the client instead (Patterns 1 and 2).

## The Matching Algorithm in Detail

When the scheduler receives an action with platform properties, matching proceeds as follows:

**Source:** [`nativelink-scheduler/src/worker_capability_index.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/worker_capability_index.rs) (`find_matching_workers`, lines 151-214) and [`nativelink-util/src/platform_properties.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-util/src/platform_properties.rs) (`is_satisfied_by`, lines 47-78)

```
For each property the action requests:
  Exact    → worker must advertise the key with the identical value
  Minimum  → worker must advertise the key; its u64 value must be >= requested
  Priority → worker must advertise the key (any value); value is NOT compared
  Ignore   → always matches; the worker need not advertise the key at all

Result = the set of workers that satisfy every requested property
```

The distinction between `Priority` and `Ignore` is the one most people get backwards. `Priority` still gates on **key presence** — a worker that does not advertise the key is filtered out; only the *value* comparison is skipped (`platform_properties.rs:116-117`, `worker_capability_index.rs:184-193`). `Ignore` is the truly permissive one: the action may request the key, but workers that lack it entirely remain eligible (`platform_properties.rs:49-50,121-123`). This is why the `InputRootAbsolutePath` property is typed `ignore` — clients set it, but no worker has to advertise it.

`Minimum` is checked in two stages: the index returns workers that *have* the key, then the caller verifies `value >= requested` at match time, because a worker's available resources change as jobs are assigned (`worker_capability_index.rs:184-193`).

If the result set is empty, the action stays queued until a matching worker appears (or times out). This is important for auto-scaling: if no worker matches, you may need to provision new workers with the right capabilities.

## Anti-Patterns

### Using tags instead of digests

```
# BAD: tag is mutable, same action hash with different toolchains
container-image: docker://toolchain:latest

# GOOD: digest is immutable, action hash changes with toolchain
container-image: docker://toolchain@sha256:abc123
```

### Using "priority" for matching-relevant properties

```json5
# BAD: ISA as priority means any worker gets any architecture action
supported_platform_properties: { ISA: "priority" }

# GOOD: ISA as exact means only matching workers get the action
supported_platform_properties: { ISA: "exact" }
```

### Not including toolchain identity in properties at all

If you rely on "all workers have the same toolchain installed" without any property expressing the toolchain version, you have:
- No cache safety (same action hash with different toolchains)
- No way to detect when a worker has a stale toolchain
- No way to do rolling toolchain updates (old and new workers coexist during deployment)

Always include at least one property that changes when the toolchain changes. It doesn't matter which approach you use — the property value just needs to be a function of the toolchain content.
