# The Execution Contract

Remote execution works because client and server agree on a contract. The contract is implicit in the protocol but never spelled out in the official spec. Here it is.

## What the Client Promises

1. **All inputs are in CAS before calling Execute.** The client must upload the complete input tree. If the worker can't find an input blob, the action fails.

2. **The Command is deterministic.** Given the same inputs and platform, the command produces the same outputs. If it doesn't, the cache will serve stale results. REAPI has no mechanism to express non-determinism — the contract assumes hermetic actions.

3. **The Platform is complete.** The platform properties in the action fully specify the execution environment requirements. The client is responsible for ensuring its toolchain matches what the worker provides.

4. **Output paths are declared.** The client declares which output files and directories the action will produce. The worker only uploads these paths. Anything else the command writes is discarded.

## What the Server Guarantees

1. **Content integrity.** Blobs retrieved from CAS by digest are byte-for-byte identical to what was uploaded — if they weren't, the hash wouldn't match. That reasoning hides one premise: client and server must agree on *which* hash function the digest names. REAPI lets a client leave `digest_function` unset (0/`UNKNOWN`); a server that then defaults to the wrong algorithm — a BLAKE3 client against a SHA256 default — voids the guarantee, because the bytes are stored under a digest computed the wrong way and a `VerifyStore` re-hash can't catch it (it re-hashes with the same wrong default). The defense is to name the digest function explicitly on every request rather than relying on a server-side default.

2. **Execution isolation.** Each action runs in its own sandbox. Actions cannot observe each other's state. (The strength of this guarantee varies — NativeLink supports Linux namespaces for PID/IPC/mount isolation.)

3. **Platform matching.** The scheduler only dispatches an action to a worker whose declared capabilities satisfy the action's platform requirements.

4. **At-least-once execution.** If the scheduler dispatches an action, it will eventually either succeed, fail, or time out. Transient failures (worker crash, network partition) result in re-dispatch.

5. **Result caching is safe to use.** If the AC returns a result for an action digest, that result is valid — it was produced by a successful execution of that exact action on a matching platform.

## What Nobody Guarantees

Here's where it gets interesting — the things the protocol explicitly does **not** guarantee:

### No toolchain identity

The platform properties are key-value string pairs. No standard says what they mean. `OSFamily: Linux` and `container-image: docker://ubuntu:22.04` are both valid, but neither actually specifies the toolchain. Two workers with identical platform properties can have different compilers, different libc versions, different everything.

This is the fundamental gap in REAPI, and it's why toolchain management is an entire part of this book.

### No ordering between actions

Actions are independent. The protocol provides no mechanism for expressing "action B depends on action A's outputs." Build systems handle this by only submitting action B after action A completes and its outputs are in CAS.

### No output determinism verification

The server trusts the worker. Two runs of the same non-deterministic action hash to *different* CAS output digests, so both land in the store as valid uploads under different keys; nothing compares them. No built-in mechanism detects or prevents this.

`VerifyStore` does not close this gap, and it is worth being precise about its scope. Wrapping a **CAS** store, it re-hashes each blob *on the write path* as the upload streams in and rejects any blob whose bytes don't match the digest the client claimed (`verify_store.rs:162`); reads pass straight through unverified (`verify_store.rs:222`). That catches corruption and lying clients at ingest — never non-determinism, since two differing-but-correctly-hashed outputs are both honest uploads. It also guards the CAS, not the AC: `verify_size`/`verify_hash` are meant to be off for the action cache and on for CAS stores (`stores.rs:960`).

### No cache invalidation

The `ActionCache` service exposes exactly two methods — `GetActionResult` and `UpdateActionResult` (`ac_server.rs:171`) — and no invalidation RPC: nothing that says "the result for this action is now wrong; stop serving it." That gap is real, but it is *not* because the AC is append-only. `UpdateActionResult` is last-writer-wins: `inner_update_action_result` serializes the `ActionResult` and calls `update_oneshot(action_digest, …)`, overwriting whatever entry was already keyed under that action digest (`ac_server.rs:121`). An entry can be *replaced*; what it cannot be is *invalidated* through the protocol.

The distinction bites in practice. Overwriting only helps if something re-executes the action and writes a fresh result — but a client that gets a cache hit won't re-execute, so a stale entry (a toolchain bug, for example) keeps being served. To actually retire a cached result you change the inputs — which changes the action hash, so you are looking up a different key — or you clear the AC out of band (LRU/TTL eviction, or wiping the store). A read-only AC endpoint (`read_only: true`, `ac_server.rs:131`) removes even the overwrite path, pinning results until eviction.

## The Hermeticity Spectrum

In practice, builds exist on a spectrum of hermeticity:

| Level | What's Fixed | Cache Safety |
|-------|-------------|--------------|
| **None** | Nothing. Actions depend on host state. | Unsafe. Cache hits may be wrong. |
| **Container** | OS and packages pinned by image tag. | Mostly safe. Image tag ≠ image content. |
| **Container + digest** | OS and packages pinned by image digest. | Safe. Content-addressed. |
| **Nix** | Everything pinned by Nix store path (content-addressed). | Safe. Bit-for-bit reproducible. |

NativeLink doesn't enforce any particular level. It's a server — it does what the client asks. But your cache hit rate and your cache correctness are entirely determined by where you sit on this spectrum.

The toolchain chapters (Part V) explain how to get to each level. Structural correctness — the toolchain's identity living *inside* the action hash by content-addressing rather than by convention — is reachable more than one way. A digest-pinned container image gets there (the `Container + digest` row above): the digest is content, and it's part of the platform, so it's part of the action hash. What Nix/LRE adds *on top* is bit-for-bit reproducibility — not merely a stable identity for the toolchain, but the ability to rebuild that exact toolchain from source anywhere. That extra guarantee is why the Nix chapters get the most attention, but it is not the only route to a correctness that is structural rather than conventional.
