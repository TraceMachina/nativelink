# Appendix C: Troubleshooting

Common errors, what they mean, and how to fix them.

## Connection Errors

### "Connection refused" / "UNAVAILABLE"

**Cause:** Client can't reach NativeLink.

**Check:**
1. Is NativeLink running? `docker ps` or `systemctl status nativelink`
2. Is the port correct? Default is 50051
3. Is the address correct? No `http://` prefix for gRPC (use `grpc://` or bare `host:port`)
4. Firewall rules? `nc -z host 50051`
5. If Docker: is the port published? `-p 50051:50051`

### "TLS handshake failed" / "CERTIFICATE_VERIFY_FAILED"

**Cause:** TLS misconfiguration.

**Check:**
1. Does the cert cover the hostname being used? (check SAN)
2. Is the CA trusted? (pass `--tls_certificate` in Bazel, set `tls = true` in Buck2)
3. Are cert and key files readable by the NativeLink process?
4. Is the cert expired? `openssl x509 -in cert.pem -noout -dates`

## Request Errors

### "NOT_FOUND" on every request

**Cause:** Instance name mismatch.

**Fix:**
- Bazel default: `instance_name: ""` (empty string)
- Buck2 default: `instance_name: "main"`
- NativeLink config must match the client's instance name on ALL services (CAS, AC, execution, capabilities, bytestream)

### "INVALID_ARGUMENT: Unknown platform property"

**Cause:** Client sent a property not listed in `supported_platform_properties`.

**Fix:** Add the property to the scheduler config:
```json5
supported_platform_properties: {
  "new-property": "priority"  // or "exact", "minimum", "ignore"
}
```

### "RESOURCE_EXHAUSTED"

**Cause:** Request exceeds server limits (usually blob size for batch operations).

**Fix:** This is typically transparent — clients should fall back to ByteStream for large blobs. If it persists, check:
- `max_bytes_per_stream` in bytestream config (0 = unlimited)
- Client batch size settings

## Execution Errors

### "DEADLINE_EXCEEDED" on Execute

**Cause:** Action exceeded timeout.

**Check:**
1. Worker's `max_action_timeout` — is it long enough?
2. Client's timeout (`--remote_timeout` in Bazel)
3. The action itself — is it actually slow or hanging?
4. Worker connectivity — did the worker disconnect mid-execution? (check scheduler logs)

### Action queued forever (no workers)

**Cause:** No worker matches the action's platform properties.

**Check:**
1. Are workers connected? Check scheduler health endpoint or logs.
2. Do worker properties match action requirements? Compare `platform_properties` in worker config with properties in the action.
3. For `exact` properties: values must match exactly (case-sensitive, whitespace-sensitive).
4. For `minimum` properties: worker value must be >= requested value.
5. Workers may have disconnected (timeout). Check `worker_timeout_s`.

The `exact` / `minimum` / `priority` / `ignore` matching vocabulary is
declared per scheduler under `supported_platform_properties` — for the
field-by-field reference of each variant see
[Appendix B: Scheduler Catalog](./scheduler-catalog.md).

### "FAILED_PRECONDITION: Missing inputs"

**Cause:** Worker can't find input blobs in CAS.

**Check:**
1. CAS store connectivity — can the worker reach the CAS?
2. CAS eviction — were the blobs evicted between upload and execution? Increase CAS size.
3. Network partition — is there a proxy/firewall between worker and CAS?
4. If using `grpc` store in worker: is the endpoint correct?

## Cache Issues

### 0% cache hit rate

**Cause:** Action hashes differ between machines.

**Diagnosis:** See [Debugging Cache Misses](../part6/debugging-cache-misses.md).

**Most common causes:**
1. Different toolchain (not captured in action hash)
2. Absolute paths in actions
3. Environment variable leakage
4. Instance name mismatch (requests go to different AC namespaces)

### Cache hits return "NOT_FOUND" for output blobs

**Cause:** AC has a result but CAS doesn't have the referenced blobs (evicted).

**Fix:**
1. Increase CAS storage (larger eviction policy)
2. Use `completeness_checking` store wrapper on AC:
   ```json5
   completeness_checking: {
     backend: { /* AC store */ },
     cas_store: "CAS_STORE_NAME"
   }
   ```
3. Use a durable CAS backend (S3) so blobs survive restarts

### Stale cache results (wrong output)

**Cause:** Non-deterministic action, or toolchain mismatch.

**Fix:**
1. Identify the non-deterministic action (timestamps, random values, hostname)
2. Make it deterministic or mark it as non-cacheable
3. If toolchain mismatch: add toolchain identity to platform properties (Part V)
4. Nuclear option: clear the AC and rebuild

## Performance Issues

### Slow uploads

**Check:**
1. Network bandwidth to CAS backend
2. Compression enabled? (reduces transfer size)
3. Large files going through batch API instead of ByteStream?
4. Client concurrency settings (`--remote_max_connections` in Bazel)

### Slow action execution

**Check:**
1. Worker CPU/memory — is it saturated?
2. Input fetch time — is the worker downloading large input trees?
3. Directory cache enabled? (worker `directory_cache`, `cas_server.rs:1233`)
4. Worker CAS has fast local tier? (filesystem, not just remote gRPC)

### High memory usage on scheduler

**Check:**
1. Number of in-flight actions — more actions = more state
2. Redis backend configured? (offloads state from memory)
3. Existence cache size (`max_count`) — too high?
4. Memory store eviction policy — is `max_bytes` appropriate?

## Startup Errors

### "Address already in use"

Another process (or previous NativeLink instance) holds the port.

```bash
lsof -i :50051  # find the process
kill <pid>       # or change the port in config
```

### "Permission denied" on file paths

NativeLink process doesn't have access to configured paths.

```bash
# Check ownership:
ls -la /data/cas/

# Fix:
chown -R nativelink:nativelink /data/
```

### "Store 'NAME' not found" during startup

A `ref_store` references a name that doesn't exist in the `stores` array. Check spelling and ensure the referenced store is defined before (or in the same config's) `stores` array.
