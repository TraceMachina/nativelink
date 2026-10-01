# REAPI from First Principles

The Remote Execution API (REAPI) is a gRPC protocol defined by the Bazel team and adopted by the industry. Its core is five services. Every remote cache and remote execution system — NativeLink, Buildbarn, EngFlow, BuildBuddy — implements these same five. A companion protocol from the same `remote-apis` repository, the Remote Asset API, adds two more (`Fetch` and `Push`); NativeLink serves it too.

Understanding REAPI is not optional. If you don't understand the protocol, you cannot debug cache misses, you cannot reason about performance, and you cannot configure NativeLink correctly.

## The Five Services

### 1. ContentAddressableStorage (CAS)

```protobuf
service ContentAddressableStorage {
  rpc FindMissingBlobs(FindMissingBlobsRequest) returns (FindMissingBlobsResponse);
  rpc BatchUpdateBlobs(BatchUpdateBlobsRequest) returns (BatchUpdateBlobsResponse);
  rpc BatchReadBlobs(BatchReadBlobsRequest) returns (BatchReadBlobsResponse);
  rpc GetTree(GetTreeRequest) returns (stream GetTreeResponse);
}
```

CAS is the content-addressed blob store. You put bytes in (keyed by their digest), you get bytes out. `FindMissingBlobs` is the critical operation — before uploading an input tree, the client asks "which of these blobs do you already have?" and only uploads the missing ones.

In NativeLink, CAS is backed by a store — any store. Memory, filesystem, S3, a composition of all three. The CAS service is a thin gRPC adapter over the store trait.

**Source:** [`nativelink-service/src/cas_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/cas_server.rs)

### 2. ActionCache (AC)

```protobuf
service ActionCache {
  rpc GetActionResult(GetActionResultRequest) returns (ActionResult);
  rpc UpdateActionResult(UpdateActionResultRequest) returns (ActionResult);
}
```

AC maps action digests to action results. This is where cache hits come from. The client computes the digest of its action (command + inputs + platform), calls `GetActionResult`, and if it gets a result back, it skips execution entirely and downloads the outputs from CAS.

`UpdateActionResult` is called after successful execution to store the result for future lookups. Some deployments disable client-side AC updates (only the server writes to AC after execution) to prevent cache poisoning.

**Source:** [`nativelink-service/src/ac_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/ac_server.rs)

### 3. ByteStream

```protobuf
service ByteStream {
  rpc Read(ReadRequest) returns (stream ReadResponse);
  rpc Write(stream WriteRequest) returns (WriteResponse);
  rpc QueryWriteStatus(QueryWriteStatusRequest) returns (QueryWriteStatusResponse);
}
```

CAS batch operations have a size limit. NativeLink advertises `max_batch_total_size_bytes` of 64 KiB (`MAX_BATCH_TOTAL_SIZE = 64 * 1024`, `capabilities_server.rs:36,132`) — not the 4 MiB you might assume from other REAPI servers. Anything larger than that must go over ByteStream. For large blobs — compiled binaries, tarballs, container images — clients use ByteStream: a chunked streaming interface for uploading and downloading blobs by digest.

The resource name encodes the digest, with a `{digest_function}` segment sitting between `blobs` and the hash:

- Read: `{instance_name}/blobs/{digest_function}/{hash}/{size}`
- Write: `{instance_name}/uploads/{uuid}/blobs/{digest_function}/{hash}/{size}`

That `{digest_function}` segment (`blake3`, `sha256`, …) is how ByteStream carries the hash algorithm: the stream has no proto field for it, unlike the structured RPCs. REAPI treats the segment as optional and NativeLink falls back to the server's configured hash when it is absent — which is exactly the silent-corruption hazard discussed in [The Digest](#the-digest), and the reason clients should name the digest function explicitly. The parser also accepts a compressed form, `.../compressed-blobs/{compressor}/{digest_function}/{hash}/{size}` (`resource_info.rs:46-80`), though NativeLink's Capabilities advertises no compressors, so clients never use it.

**Source:** [`nativelink-service/src/bytestream_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/bytestream_server.rs)

### 4. Execution

```protobuf
service Execution {
  rpc Execute(ExecuteRequest) returns (stream Operation);
  rpc WaitExecution(WaitExecutionRequest) returns (stream Operation);
}
```

This is remote execution. The client sends an `ExecuteRequest` containing the action digest, and gets back a stream of `Operation` messages tracking progress. Each `Operation` carries an `ExecuteOperationMetadata` whose `stage` walks the `ExecutionStage.Value` enum: `CACHE_CHECK` → `QUEUED` → `EXECUTING` → `COMPLETED` (`build.bazel.remote.execution.v2.pb.rs:1064-1075`; the `UNKNOWN` sentinel is never emitted). `CACHE_CHECK` is first for a reason — before touching a worker the server re-checks the ActionCache, so a request that raced another build (or hit a warm cache the client skipped) is served without executing. NativeLink models the same set internally as `ActionStage` (`action_messages.rs:757`).

NativeLink's execution service forwards the request to the scheduler, which dispatches to a matching worker. The operation stream is held open (long-polling) until the worker completes.

**Source:** [`nativelink-service/src/execution_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/execution_server.rs)

### 5. Capabilities

```protobuf
service Capabilities {
  rpc GetCapabilities(GetCapabilitiesRequest) returns (ServerCapabilities);
}
```

Feature negotiation. The client asks "what do you support?" and the server responds with digest functions, max batch sizes, supported compressors, execution priority ranges, etc. Clients use this to adapt their behavior.

NativeLink's `GetCapabilities` returns a fixed, honest set (`capabilities_server.rs:95-152`):

- **`digest_functions`**: `[SHA256, BLAKE3]` — advertised in both `cache_capabilities` and, when execution is configured, `execution_capabilities` (`capabilities_server.rs:116-119,124-127`). These are the only two `DigestHasherFunc` implements.
- **`max_batch_total_size_bytes`**: 64 KiB (`capabilities_server.rs:132`).
- **`supported_compressors`** and **`supported_batch_update_compressors`**: both empty — NativeLink advertises **no** compressor (`capabilities_server.rs:134-135`). Blobs move uncompressed.
- **`action_cache_update_capabilities.update_enabled`**: `true` (`capabilities_server.rs:128-130`) — clients may write to the ActionCache.
- **`symlink_absolute_path_strategy`**: `DISALLOWED` (`capabilities_server.rs:133`).
- **API version range**: low `2.0.0`, high `2.3.0`, no deprecated version (`capabilities_server.rs:138-150`).
- **`execution_capabilities`** is present only for instances wired to a scheduler; when present it advertises `exec_enabled: true` and a priority range of `0..=i32::MAX` (`capabilities_server.rs:105-120`). Its scalar `digest_function` is the server's configured default (SHA256 unless overridden).

**Source:** [`nativelink-service/src/capabilities_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/capabilities_server.rs)

## The Remote Asset API (companion protocol)

Alongside the five core services, the same `remote-apis` repository defines the **Remote Asset API** — two services that map an external URI to content in CAS:

```protobuf
service Fetch {
  rpc FetchBlob(FetchBlobRequest) returns (FetchBlobResponse);
  rpc FetchDirectory(FetchDirectoryRequest) returns (FetchDirectoryResponse);
}

service Push {
  rpc PushBlob(PushBlobRequest) returns (PushBlobResponse);
  rpc PushDirectory(PushDirectoryRequest) returns (PushDirectoryResponse);
}
```

`Fetch` resolves a URI — an `http(s)://` tarball, for instance — into CAS and returns a blob digest that a client can drop straight into an action's `input_root_digest`. `Push` records the reverse mapping, associating a URI with content already in CAS so a later `Fetch` can find it. NativeLink serves `FetchBlob` through `FetchServer` and `PushBlob` through `PushServer`; `FetchDirectory` ([`fetch_server.rs:164`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/fetch_server.rs)) and `PushDirectory` ([`push_server.rs:157`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/push_server.rs)) are currently unimplemented. In config they appear as the `fetch` and `push` service entries (`FetchConfig.fetch_store`, `PushConfig.push_store`).

**Source:** [`nativelink-service/src/fetch_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/fetch_server.rs), [`nativelink-service/src/push_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/push_server.rs)

## Instance Names

Every REAPI request carries an `instance_name` string. This is an opaque namespace — the server can use it to route to different storage backends, apply different policies, or isolate tenants.

NativeLink maps instance names to service configurations in the server config. Buck2 requires `instance_name: "main"` by convention. Bazel defaults to empty string but can be configured with `--remote_instance_name`.

## The Digest

The fundamental unit of identity in REAPI is the `Digest`:

```protobuf
message Digest {
  string hash = 1;      // hex-encoded hash
  int64 size_bytes = 2; // uncompressed size
}
```

Hash function is negotiated via Capabilities (`SHA256` is standard, `BLAKE3` is supported by NativeLink). The size is part of the identity — it lets the server pre-allocate and detect corruption without reading the full blob.

### A sharp edge: the unset digest function

REAPI has a sharp edge here worth internalizing. On the structured RPCs the digest function is a proto enum field, and an unset field decodes as `UNKNOWN` — the protobuf default. On ByteStream it is the optional resource-name segment described above, and an omitted segment is the exact same "unset" signal. The server resolves both cases by falling back to its configured default hash. If a `BLAKE3` client forgets to declare its digest function and the server defaults to `SHA256`, the server hashes the client's bytes — including output `Directory` Merkle trees — with the wrong algorithm. Nothing errors; the cache just fills with entries under mismatched keys. That is silent corruption.

The defense is on the client: name the digest function explicitly on every request — both the structured RPCs and the ByteStream resource name — so the server never has to guess. Anything that speaks `BLAKE3` should be especially careful here, since the fallback default is `SHA256`.

**Source:** [`nativelink-util/src/digest_hasher.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-util/src/digest_hasher.rs)

## How It All Fits Together

A complete remote execution flow:

1. Client builds a `Command` proto (argv, env, output paths)
2. Client builds an input `Directory` Merkle tree
3. Client uploads missing blobs to CAS via `FindMissingBlobs` + `BatchUpdateBlobs`/`ByteStream.Write`
4. Client constructs an `Action` (command digest + input root digest + platform)
5. Client checks AC: `GetActionResult(action_digest)` — if hit, done
6. Client calls `Execute(action_digest)` — receives operation stream
7. Scheduler dispatches to worker
8. Worker calls `FindMissingBlobs` on CAS (for its local cache), downloads inputs
9. Worker executes the command
10. Worker uploads outputs to CAS
11. Worker reports `ActionResult` to scheduler
12. Scheduler stores result in AC, forwards to client
13. Client downloads outputs from CAS

Steps 3-6 are the client's responsibility. Steps 7-12 are NativeLink's. Step 13 is the client again.
