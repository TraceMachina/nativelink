# Bazel

Bazel is the most common REAPI client. Its remote execution support is mature, well-documented, and works out of the box with NativeLink. This chapter covers the configuration, the gotchas, and the optimization paths.

## Basic Configuration

Remote caching only (no execution):

```bash
# .bazelrc
build --remote_cache=grpc://nativelink:50051
build --remote_upload_local_results=true
```

Remote caching and execution:

```bash
# .bazelrc
build --remote_cache=grpc://nativelink:50051
build --remote_executor=grpc://nativelink:50051
build --remote_default_exec_properties=cpu_count=1
build --remote_default_exec_properties=OSFamily=Linux
```

That's it for a basic setup. Bazel handles the REAPI protocol — `FindMissingBlobs`, `Execute`, `ByteStream` — automatically.

## Instance Names

NativeLink routes requests by instance name. Bazel defaults to empty string. You can set it:

```bash
build --remote_instance_name=main
```

This must match the `instance_name` in the NativeLink server config:

```json5
services: {
  cas: [{ instance_name: "main", cas_store: "CAS_STORE" }],
  ac: [{ instance_name: "main", ac_store: "AC_STORE" }],
  execution: [{ instance_name: "main", cas_store: "CAS_STORE", scheduler: "SCHEDULER" }]
}
```

If you omit `--remote_instance_name`, the NativeLink config should use `instance_name: ""` (empty string).

## Platform Configuration

For remote execution, you need to tell Bazel what platform the remote workers provide:

```python
# platforms/BUILD.bazel
platform(
    name = "linux_x86_64",
    constraint_values = [
        "@platforms//os:linux",
        "@platforms//cpu:x86_64",
    ],
    exec_properties = {
        "container-image": "docker://toolchain@sha256:abc123...",
        "OSFamily": "Linux",
        "ISA": "x86-64",
    },
)
```

```bash
# .bazelrc
build --extra_execution_platforms=//platforms:linux_x86_64
build --host_platform=//platforms:linux_x86_64
```

The `exec_properties` map becomes the action's platform properties in the `Execute` RPC. These must match the scheduler's `supported_platform_properties` configuration.

## Digest Function

Select the content hash with Bazel's `--digest_function` startup flag:

```bash
startup --digest_function=blake3
```

`--digest_function` is a Bazel *startup* flag, not a `build` flag — it changes how
the Bazel server addresses every blob, so it has to be set before that server
starts. NativeLink's Capabilities service advertises both SHA256 and BLAKE3
(`capabilities_server.rs:116-127`), so either works; BLAKE3 is much faster than the
SHA256 default. Pick one and set it consistently across every client and worker:
if a client omits the digest function, REAPI carries it as the proto default `0`
(`UNKNOWN`), and mixing SHA256 and BLAKE3 clients against one deployment without
pinning the function invites cross-hashing surprises.

## NativeLink Integration Tests

The repository includes a complete test of Bazel remote execution and caching:

**Source:** [`integration_tests/simple_remote_execution_test.sh`](https://github.com/TraceMachina/nativelink/blob/main/integration_tests/simple_remote_execution_test.sh)

The relevant `.bazelrc` flags:

```bash
# From .bazelrc (lines 61-65)
build:self_test --remote_cache=grpc://127.0.0.1:50051
build:self_execute --remote_executor=grpc://127.0.0.1:50052
build:self_execute --remote_default_exec_properties=cpu_count=1
```

**Source:** [`.bazelrc`](https://github.com/TraceMachina/nativelink/blob/main/.bazelrc)

## Toolchain Integration

### With LRE

Local Remote Execution (LRE) generates a Bazel toolchain from the same Nix
closure the workers run, so a locally-built action and a remotely-executed one
resolve to byte-identical toolchains — the precondition for cross-machine cache
hits. Import the generated flags:

```bash
# Import generated LRE config (created by the nix develop shell hook)
try-import %workspace%/lre.bazelrc
```

The generated `lre.bazelrc` wires up **both** the C++ (`lre-cc`) and Rust
(`lre-rs`) toolchains — LRE is not C++-only:

```bash
build:linux --define=EXECUTOR=remote
build:linux --extra_execution_platforms=@local-remote-execution//rust/platforms:x86_64-unknown-linux-gnu,@local-remote-execution//rust/platforms:x86_64-unknown-linux-musl,@local-remote-execution//generated-cc/config:platform
build:linux --action_env=BAZEL_DO_NOT_DETECT_CPP_TOOLCHAIN=1
build:linux --extra_toolchains=@local-remote-execution//generated-cc/config:cc-toolchain
build:linux --extra_toolchains=@local-remote-execution//rust:rust-x86_64-linux
build:linux --extra_toolchains=@local-remote-execution//rust:rustfmt-x86_64-linux
build:linux --platforms=@local-remote-execution//rust/platforms:x86_64-unknown-linux-musl
```

The `:linux` config prefix comes from `lre.prefix = "linux"` in the NativeLink
flake (`flake.nix:572-575`); a downstream flake that leaves the prefix empty gets
bare `build --...` lines that apply to every build with no `--config`. The C++
toolchain is `x86_64-linux` only, while the Rust toolchain is multi-platform
(`aarch64`/`x86_64` × `linux-gnu`/`linux-musl`, plus Darwin). For the full
derivation — image tags, `container-image` versus `lre-rs` platform properties,
and the Nix flake wiring — see [LRE on Nix](../part8/lre-nix.md).

### With zig-cc

```bash
build --config=zig-cc
build --remote_executor=grpc://nativelink:50051
```

### With toolchains_llvm

```bash
build --config=llvm
build --remote_executor=grpc://nativelink:50051
```

See the [toolchain-examples](https://github.com/TraceMachina/nativelink/tree/main/toolchain-examples) directory for complete working configurations of each approach.

## Optimization Flags

### Compression

```bash
build --experimental_remote_cache_compression
build --experimental_remote_cache_compression_threshold=100
```

These flags ask Bazel to transfer blobs with the REAPI compressed-blobs encoding.
**NativeLink does not implement it.** The Capabilities service advertises an empty
compressor list — `supported_compressors: vec![]` and
`supported_batch_update_compressors: vec![]` (`capabilities_server.rs:134-135`) —
and batch reads always answer with the `Identity` compressor
(`cas_server.rs:228`). A REAPI-conformant client checks `supported_compressors`
before compressing, so these flags do not negotiate compression against
NativeLink; treat them as a no-op here. For bandwidth, reach for
`--remote_download_minimal` (below) — a far bigger win regardless.

### Remote Output Mode

```bash
build --remote_download_minimal    # don't download outputs you don't need
build --remote_download_toplevel   # only download top-level outputs
```

`remote_download_minimal` (a.k.a. "Build without the Bytes") is the biggest single optimization. Bazel skips downloading intermediate outputs — it trusts that they're in CAS and only downloads final outputs. For large builds, this reduces bandwidth by 10-100x.

### Disk Cache + Remote Cache

```bash
build --disk_cache=/tmp/bazel-disk-cache
build --remote_cache=grpc://nativelink:50051
```

Bazel checks the local disk cache before hitting the remote. This eliminates network round-trips for recently-built artifacts and gives you cache hits even when offline.

### Build Event Protocol

```bash
build --bes_backend=grpc://nativelink:50051
build --bes_results_url=https://your-dashboard.example.com/invocation/
```

NativeLink's experimental BEP server can ingest build events for monitoring and debugging.

**Source:** [`nativelink-service/src/bep_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/bep_server.rs)

## TLS Configuration

For production deployments:

```bash
build --remote_cache=grpcs://nativelink:50051
build --tls_certificate=/path/to/ca.crt
build --tls_client_certificate=/path/to/client.crt
build --tls_client_key=/path/to/client.key
```

NativeLink TLS config:

```json5
listener: {
  http: {
    socket_address: "0.0.0.0:50051",
    tls: {
      cert_file: "/certs/server.crt",
      key_file: "/certs/server.key",
      client_ca_file: "/certs/ca.crt",   // presence turns on mTLS
      client_crl_file: "/certs/crl.pem"  // optional revocation list
    }
  }
}
```

`TlsConfig` is `deny_unknown_fields` and has exactly four keys — `cert_file`,
`key_file`, `client_ca_file`, and `client_crl_file` (`cas_server.rs:778-796`).
No `client_auth_optional` toggle exists: mTLS is on whenever `client_ca_file`
is set and off otherwise, and `client_crl_file` names an optional certificate
revocation list. Any other key is a hard startup error — `client_auth_optional:
false` aborts loading — so the process fails fast on a bad `tls` block rather than
starting with it silently ignored.

## Common Issues

### "NOT_FOUND: Action result not found in cache"

The AC returned a result referencing CAS blobs that no longer exist (evicted). Solutions:
- Increase CAS size (or use durable backend like S3)
- Use `completeness_checking` store wrapper on the AC
- Reduce eviction pressure by deduplicating with `dedup` store

### "DEADLINE_EXCEEDED" on Execute

The action exceeded its timeout. Check:
- `max_action_timeout_s` in the worker config (`LocalWorkerConfig`,
  `cas_server.rs:1124`; accepts the alias `max_action_timeout`, default 20 minutes,
  `local_worker.rs:74`). The worker rejects any action whose requested timeout
  exceeds this cap with `InvalidArgument` (`running_actions_manager.rs:2905`).
- `--remote_timeout` in Bazel flags (default: 600s)
- The action itself (is it actually hanging?)

### "UNAVAILABLE: Connection refused"

Bazel can't reach NativeLink. Check:
- Is NativeLink running? (`curl grpc://host:port` won't work — use `grpcurl`)
- Is the address correct? (no `http://` prefix for gRPC)
- Are there firewall rules blocking the port?
- If using TLS, are certificates valid and trusted?

### Low cache hit rate

Almost always a toolchain problem. Run `bazel aquery //target` on two machines and compare the action keys. If they differ, check:
- Are platform properties identical?
- Is the toolchain binary identical? (check paths, versions, hashes)
- Are environment variables leaking? (`--incompatible_strict_action_env`)
- Is there non-deterministic input ordering? (glob patterns, genrules)
