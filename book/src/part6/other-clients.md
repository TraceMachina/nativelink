# Other Clients

NativeLink speaks REAPI. Any client that speaks REAPI works with NativeLink — not just Bazel and Buck2. This chapter covers the other clients: Goma/Reclient (Chromium), recc (CMake), Pants, and custom integrations.

## Goma / Reclient (Chromium)

Google's Chromium project uses Goma (deprecated) and its successor Reclient for distributed compilation. Both speak REAPI for the CAS and execution layer.

NativeLink has been validated with Chromium builds:

**Source:** [`deploy/chromium-example/`](https://github.com/TraceMachina/nativelink/tree/main/deploy/chromium-example)

Reclient configuration:
```
RBE_service=nativelink.example.com:50051
RBE_instance=main
RBE_platform=container-image=docker://chromium-toolchain@sha256:...
```

Chromium's build system wraps `clang` calls with Reclient, which handles hashing, cache lookup, and remote dispatch. From NativeLink's perspective, it's just another REAPI client.

## recc (CMake)

recc is a remote execution client for CMake. It wraps compiler invocations and turns them into REAPI Execute calls. This lets you add remote caching and execution to CMake projects without changing the build system.

```bash
# Use recc as the compiler wrapper
export CC="recc cc"
export CXX="recc c++"

# Configure recc to use NativeLink
export RECC_SERVER=nativelink.example.com:50051
export RECC_INSTANCE=main
export RECC_CACHE_ONLY=1  # remote cache only (no execution)

# Run CMake as usual
cmake -B build && cmake --build build
```

recc computes the action hash from the preprocessed source, command arguments, and a platform descriptor. It checks the AC, and on miss either runs locally (cache-only mode) or submits for remote execution.

## Pants

Pants (v2+) supports REAPI remote caching and execution. Configuration is in `pants.toml`:

```toml
# pants.toml
[GLOBAL]
remote_store_address = "grpc://nativelink.example.com:50051"
remote_execution_address = "grpc://nativelink.example.com:50051"
remote_instance_name = "main"
remote_cache_read = true
remote_cache_write = true
```

Pants' process execution layer translates internal "Process" objects to REAPI Execute calls. The same NativeLink server config works for Pants as for Bazel.

## BuildStream

BuildStream is a full REAPI client, not a maybe. It drives the whole remote-execution triple — execution, action cache, and CAS (storage) — over REAPI, and it also keeps a CAS-backed artifact cache. NativeLink ships a working end-to-end integration test: [`integration_tests/buildstream/`](https://github.com/TraceMachina/nativelink/tree/main/integration_tests/buildstream) starts a live NativeLink from `buildstream_cas.json5`, builds the `hello.bst` element against it, and fails the run unless BuildStream prints `SUCCESS Build` and NativeLink logs no `ERROR` (`buildstream-with-nativelink-test.nix:15-40`).

The client config wires every REAPI surface to one NativeLink endpoint (`buildstream.conf`):

```yaml
# buildstream.conf
artifacts:
  servers:
  - url: http://localhost:50051   # CAS-backed artifact cache
    push: true
remote-execution:
  execution-service:
    url: http://localhost:50051
  action-cache-service:
    url: http://localhost:50051
  storage-service:
    url: http://localhost:50051
```

BuildStream's `instance-name` is configurable and defaults to the empty string; the test leans on that default, so its server config serves the `""` instance (`buildstream_cas.json5`). From NativeLink's side BuildStream is just another REAPI client — the same `cas`, `ac`, `execution`, and `capabilities` services that serve Bazel serve it too.

## Custom REAPI Clients

If you're building your own CI pipeline, code generation system, or build tool, you can speak REAPI directly. The protocol is defined in protobuf:

- [`build/bazel/remote/execution/v2/remote_execution.proto`](https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto)

Minimal client workflow:
1. Serialize your inputs as blobs, compute digests
2. Upload missing blobs (`FindMissingBlobs` → `BatchUpdateBlobs`/`ByteStream.Write`)
3. Construct an `Action` proto (command digest + input root digest + platform)
4. Check AC: `GetActionResult(action_digest)`
5. On miss: `Execute(action_digest)` → wait for operation completion
6. Download outputs via `ByteStream.Read`

Libraries for REAPI clients exist in:
- **Rust:** `tonic` + generated protos (see NativeLink's `nativelink-proto` crate)
- **Go:** `bazelbuild/remote-apis-sdks`
- **Python:** `grpcio` + generated stubs
- **C++:** Bazel's `remote_execution` library

## Universal Configuration

Regardless of client, the NativeLink server config is the same. The only client-specific consideration is `instance_name`:

| Client | Default Instance Name |
|--------|----------------------|
| Bazel | `""` (empty string) |
| Buck2 | `"main"` |
| Reclient | Configurable, often `"default"` |
| recc | Configurable |
| Pants | Configurable |
| BuildStream | Configurable, defaults to `""` |

If you need to support multiple clients, configure multiple instance names in NativeLink:

```json5
services: {
  cas: [
    { instance_name: "", cas_store: "CAS_STORE" },
    { instance_name: "main", cas_store: "CAS_STORE" },
    { instance_name: "default", cas_store: "CAS_STORE" }
  ],
  // ... same for ac, execution, capabilities, bytestream
}
```

All instance names can point to the same underlying stores. They're just routing labels.

### Digest-function safety in a mixed fleet

Instance names route requests; they do **not** pin a digest function. A single NativeLink instance advertises both SHA256 and BLAKE3 and serves whichever a client asks for (`capabilities_server.rs:116-127`), and its advertised *default* is SHA256 (`default_digest_hasher_func()` returns `DigestHasherFunc::Sha256`, `digest_hasher.rs:55-57`). That default is a trap for a mixed fleet.

The failure mode: a client that hashes with **BLAKE3** but omits the `digest_function` field sends it as the protobuf zero value (`UNKNOWN`). NativeLink treats zero as "unset" and falls back to `default_digest_hash_function` — SHA256 — so those BLAKE3 blobs are addressed as if they were SHA256. Output `Directory` trees end up hashed under the wrong algorithm, and the corruption is silent.

The defense is on the client side: pin the digest function explicitly on every client (Bazel's `--digest_function=blake3` startup flag, and the equivalent for other REAPI clients) so no request ever rides on the server's default. Keep the function uniform across the whole fleet, and never mix a BLAKE3 client that omits the field with a SHA256 default.

## Client Feature Matrix

| Feature | Bazel | Buck2 | Reclient | recc | Pants |
|---------|:-----:|:-----:|:--------:|:----:|:-----:|
| Remote cache | Yes | Yes | Yes | Yes | Yes |
| Remote execution | Yes | Yes | Yes | Yes | Yes |
| Build without the bytes | Yes | Yes | No | No | Yes |
| zstd `compressed-blobs` | — | — | — | — | — |
| BEP | Yes | No | No | No | No |
| Persistent workers | Yes | Yes | No | No | No |
| Hybrid local/remote | Basic | Advanced | Yes | Basic | Yes |
| mTLS | Yes | Yes | Yes | Yes | Yes |

On compression that row is uniform for a reason. REAPI negotiates `compressed-blobs` (zstd) transfer through the `supported_compressors` field of the Capabilities response, and NativeLink returns that list **empty** — `supported_compressors: vec![]` and `supported_batch_update_compressors: vec![]` (`capabilities_server.rs:134-135`). A client that honors capabilities negotiation therefore never sends or requests zstd-compressed blobs, whatever it supports locally; the `—` means "the server declines," not "the client can't." The only compression NativeLink offers is gRPC **transport** gzip, enabled per-server with the `compression` block (`send_compression_algorithm` / `accepted_compression_algorithms`, gzip only — there is no zstd HTTP transport). That is a wire-level option, independent of the client and of REAPI compressed-blobs; see [the config reference](../appendix/config-reference.md#serverconfig).

Every other row is client-side capability. NativeLink serves those on the server side; the compression row is the one place where the answer is the server's, and the server says no.
