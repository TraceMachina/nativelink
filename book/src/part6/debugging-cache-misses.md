# Debugging Cache Misses

Your cache hit rate is 12%. It should be 95%. Something is different between the two machines computing the same action, and the action hashes are diverging. This chapter is a systematic debugging guide.

## The Debugging Process

Once the server config is sound ([Step 0](#step-0-rule-out-a-broken-config)), cache misses happen for essentially one reason: the action digest on machine A differs from the action digest on machine B. The action digest is the hash of the `Action` proto — conceptually `hash(Command, InputRootDigest, Platform)` — so something in the command, the inputs, or the platform is different. One subtlety hides behind that formula: the *hash function itself* is part of the cache identity, so two clients hashing identical content with different functions diverge just as surely (see [Cause: Digest-function divergence](#cause-digest-function-divergence)).

## Step 0: Rule Out a Broken Config

Before you diff action digests across machines, confirm the server config itself is not the cause. A dangling store reference, a stale field name, or an unintended digest-function default can degrade or silently disable caching without any obvious error at the client. The server validates the config at startup, so boot the binary against it (in CI, say) and watch for a non-zero exit:

```console
$ nativelink nativelink.json5
```

At startup the server parses every block, enforces `deny_unknown_fields`, and resolves every store reference before it begins serving — a bad config exits non-zero rather than coming up half-working. It catches the config-level causes of "misses" that no amount of action-hash diffing will explain:

- **A dangling store reference.** An `ac_store` or `cas_store` that names a store which does not exist fails the reference check, and the service never serves that cache.
- **A stale or misspelled field.** Every config struct is `deny_unknown_fields`, so `cache_metric` (missing the trailing `s`) or a renamed key is a hard error, not a silently-ignored no-op.
- **A digest-function default you did not intend.** `global.default_digest_hash_function` decides how the server treats requests that omit the digest function — a common cause of a whole fleet missing the cache (see [Cause: Digest-function divergence](#cause-digest-function-divergence)).

If the server exits with a config error at startup, fix that before touching anything else: the "cache miss" is a configuration bug, not a hermeticity bug.

## Step 1: Identify the Diverging Action

### Bazel

```bash
# On machine A:
bazel aquery //target:name --output=jsonproto > /tmp/actions_a.json

# On machine B:
bazel aquery //target:name --output=jsonproto > /tmp/actions_b.json

# Compare:
diff <(jq '.actions[0]' /tmp/actions_a.json) <(jq '.actions[0]' /tmp/actions_b.json)
```

Alternatively, use Bazel's execution log:

```bash
bazel build //target --execution_log_json_file=/tmp/exec_log.json
```

The execution log records every action with its inputs, command, and platform. Compare logs between machines.

### Buck2

```bash
# Show action details:
buck2 aquery "//target:name" --output-format=json
```

Buck2's `aquery` shows the command, inputs, and platform for each action. Compare outputs across machines.

## Step 2: Identify What's Different

The difference is always in one of three places:

### A. Command Differences

The `Command` proto includes:
- `arguments` (argv)
- `environment_variables`
- `output_paths`
- `working_directory`
- `platform`

Common command divergences:
- **Absolute paths in argv.** `/home/alice/project/...` vs `/home/bob/project/...`. Fix: use relative paths.
- **Environment variable leakage.** `HOME`, `USER`, `TMPDIR` differ between machines. Fix: `--incompatible_strict_action_env` (Bazel) or explicit env filtering.
- **Toolchain path differs.** `/usr/bin/gcc` resolves to different binaries. Fix: use a hermetic toolchain (Part V).
- **Output path ordering.** Glob patterns expand in filesystem order, which may differ. Fix: sort explicitly.

### B. Input Differences

The `InputRootDigest` is the Merkle tree of the action's input files. If any input file differs, the root digest differs.

Common input divergences:
- **Generated files differ.** A genrule that embeds timestamps, hostnames, or random values. Fix: make generators deterministic.
- **Source file encoding.** CRLF vs LF across operating systems. Fix: `.gitattributes` with `* text=auto eol=lf`.
- **Different dependency versions.** Lock files diverge between machines. Fix: commit lock files, ensure deterministic resolution.
- **Toolchain binaries in input tree.** If the toolchain is an input (hermetic approach), different toolchain versions = different inputs. Fix: pin the toolchain version.

### C. Platform Differences

The `Platform` in the action includes `exec_properties` / platform properties. If these differ between machines:
- Different `container-image` value
- Different resource requirements (`cpu_count`)
- Extra/missing properties

## Step 3: Common Root Causes and Fixes

### Cause: Digest-function divergence

**Symptom:** Two clients build the *same* action from the *same* inputs and never share a cache entry. Neither one is "wrong" — each hits the cache against its own prior builds, but never against the other's.

**Diagnosis:** The two clients hash with different digest functions. An action's cache identity is not just its content — it is `(instance_name, digest_function, action_digest)`. The `action_digest` is the hash of the `Action` proto, and BLAKE3 and SHA256 produce completely different hashes of the same bytes. REAPI even namespaces blob addresses by function (`.../blobs/{digest_function}/{hash}/{size}`), so a BLAKE3 client and a SHA256 client occupy disjoint address spaces. Same action, different function, different key, guaranteed miss. This is orthogonal to the three content axes in Step 2: nothing in the command, the inputs, or the platform changed — only the hash function did.

The insidious variant is a client that omits `digest_function` entirely (sends the proto default `0` / `UNKNOWN`). The server substitutes `global.default_digest_hash_function` (`SHA256` unless you changed it) for such requests (`default_digest_hasher_func`, `nativelink-util/src/digest_hasher.rs:52-61`). So a BLAKE3 client that forgets to set the field has its outputs hashed as SHA256, its `Directory` trees addressed under the wrong function, and cache entries no correctly-configured BLAKE3 client will ever match.

**Fix:** Standardize on one digest function across every client and the execution service, and set it explicitly on every request. Set `global.default_digest_hash_function` to that same function so the server's fallback for a request that omits the field matches your fleet rather than silently diverging:

```json5
{
  global: {
    default_digest_hash_function: "blake3", // match your clients
  },
}
```

Modern Bazel and Buck2 set the digest function on every request, so the omitted-field case is mostly a hazard with older or hand-rolled clients. The surest defense is to pin one function fleet-wide and keep `default_digest_hash_function` aligned with it.

### Cause: Host toolchain leaking into action hash

**Symptom:** Different machines produce different action hashes for the same source.
**Diagnosis:** The action's command references a host-installed binary (`/usr/bin/gcc`) that isn't captured in the action hash.
**Fix:** Use a hermetic toolchain (zig-cc, Nix/LRE, container image).

### Cause: Timestamps in generated code

**Symptom:** Actions that depend on generated files always miss the cache.
**Diagnosis:** A code generator embeds `__DATE__`, `__TIME__`, or uses `Date.now()`.
**Fix:** Strip timestamps from code generators. Use `SOURCE_DATE_EPOCH` for reproducible timestamps.

### Cause: Absolute paths

**Symptom:** Same code, same toolchain, different cache keys between users.
**Diagnosis:** Build tool embeds workspace root (`/home/username/...`) in command args or environment.
**Fix:**
- Bazel: `--incompatible_strict_action_env`, ensure sandboxed execution
- Buck2: use relative paths in rule implementations
- Both: avoid `ctx.workspace_root` in action commands

### Cause: Non-hermetic environment variables

**Symptom:** Cache hits in CI but not locally (or vice versa).
**Diagnosis:** Actions inherit env vars from the shell that differ between environments.
**Fix:**
- Bazel: `--incompatible_strict_action_env` (blocks most env inheritance)
- Buck2: explicit environment in `CommandExecutorConfig`
- Both: audit which env vars the action actually needs

### Cause: Platform property mismatch

**Symptom:** Actions never hit the cache, even when built twice on the same machine.
**Diagnosis:** Platform properties change between invocations (mutable image tag, dynamic value).
**Fix:** Pin all platform property values. Never use `:latest` or unpinned tags.

### Cause: Action cache eviction

**Symptom:** Cache hit rate decreases over time. Old actions always miss.
**Diagnosis:** AC or CAS is too small; entries are evicted before reuse.
**Fix:** Increase store sizes, add durable backend (S3), use `existence_cache` to reduce CAS churn.

## Step 4: Verify the Fix

After fixing the root cause:

```bash
# Build on machine A:
bazel build //target --execution_log_json_file=/tmp/exec_a.json

# Build on machine B:
bazel build //target --execution_log_json_file=/tmp/exec_b.json

# Verify action digests match. The JSON execution log is a *stream* of
# SpawnExec objects (one per spawn), not a top-level array — so apply the
# filter directly, with no `.[]`. Each SpawnExec carries a `digest` object
# ({ hash, sizeBytes, hashFunctionName }); the action's identity is its hash:
jq -r '.digest.hash' /tmp/exec_a.json | sort > /tmp/keys_a
jq -r '.digest.hash' /tmp/exec_b.json | sort > /tmp/keys_b
diff /tmp/keys_a /tmp/keys_b
# Should be empty (all digests match)
```

For Buck2, compare `buck2 aquery` output:
```bash
buck2 aquery "//..." --output-format=json | jq '.[] | .digest'
```

## The Cache Hit Rate Formula

```
hit_rate = cache_hits / (cache_hits + cache_misses)
```

Target hit rates:
- **< 50%:** Something is fundamentally broken. Probably toolchain mismatch.
- **50-80%:** Partial sharing. Some actions are hermetic, some aren't. Find the non-hermetic ones.
- **80-95%:** Good. Remaining misses are likely cold cache (first build) or genuinely changed inputs.
- **> 95%:** Excellent. You have a well-configured hermetic build.

If you're using NativeLink's `cache_metrics` store wrapper, hit/miss rates are emitted as low-cardinality OpenTelemetry cache-operation metrics for the wrapped store. The wrapper takes two required fields — `cache_type`, a low-cardinality label so the metrics can be told apart (for example `cas` or `ac`), and `backend`, the store to wrap (`CacheMetricsSpec`, `nativelink-config/src/stores.rs:622-628`, `deny_unknown_fields`):

```json5
{
  name: "AC_WITH_METRICS",
  cache_metrics: {
    cache_type: "ac",
    backend: { ref_store: { name: "AC_STORE" } },
  },
}
```

`cache_type` is not optional: omit it and the config fails to parse with `missing field 'cache_type'`, so the server exits at startup before serving. The wrapper is opt-in — a store you do not wrap pays none of its hot-path timing cost (`stores.rs:53-58`). These metrics appear in your Prometheus/Grafana dashboard (see the Observability chapter).

## See Also

For symptom-first entries keyed to the exact error text a client reports — "0% cache hit rate", "Cache hits return NOT_FOUND for output blobs", and "Stale cache results (wrong output)" — see the Cache Issues section of [Appendix C: Troubleshooting](../appendix/troubleshooting.md). It cross-references back to the diagnostic process here.
