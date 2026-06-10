# REAPI Wire Compression (Zstd Compressed-Blobs) Implementation Plan

**Goal:** Add REAPI wire compression support to NativeLink so that Bazel clients using `--remote_cache_compression` can successfully compress blobs over the WAN, reducing bandwidth for remote builds.

**Architecture:** Wire compression sits in the gRPC service layer (ByteStream server, CAS server), not in the store layer. On upload (write), the server decompresses compressed-blobs data before storing the raw bytes. On download (read), the server compresses raw bytes from the store before sending. The store always holds uncompressed data. This is orthogonal to the existing at-rest CompressionStore (LZ4).

**Tech Stack:** Rust, zstd crate, tonic gRPC, prost protobuf types (already generated), existing ResourceInfo parser.

**Design Decisions (resolved):**
1. Support zstd only initially (YAGNI -- Bazel uses zstd exclusively with `--remote_cache_compression`)
2. Compress/decompress in the service layer before/after the store, NOT in the store layer
3. Do NOT modify CompressionStore -- it uses LZ4 with a custom frame format for at-rest storage
4. Config-driven: add `supported_wire_compressors` to CapabilitiesConfig, threaded through to CapabilitiesServer
5. The `expected_size` in ResourceInfo for compressed-blobs refers to the UNCOMPRESSED size (per REAPI spec)

**Scope:**
- Phase 1: Config + capability advertisement
- Phase 2: ByteStream read/write zstd compression
- Phase 3: BatchUpdateBlobs/BatchReadBlobs zstd compression
- Phase 4: Integration testing

---

## Phase 1: Config and Capability Advertisement

### Task 1.1 — Add `WireCompressor` enum to config

**File:** `nativelink-config/src/cas_server.rs`

Add a new enum `WireCompressor` below the `HttpCompressionAlgorithm` enum (around line 73):

```rust
/// Wire compression algorithm for REAPI compressed-blobs.
/// This is the compression applied on the gRPC wire between client and server,
/// as described in the REAPI compressed-blobs specification.
/// This is distinct from at-rest compression (CompressionStore/LZ4).
#[derive(Deserialize, Serialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum WireCompressor {
    /// No wire compression (default, backwards compatible).
    #[default]
    Identity,
    /// Zstandard compression. This is the only algorithm Bazel uses with
    /// `--remote_cache_compression`.
    Zstd,
}
```

**Commit:** `feat(config): add WireCompressor enum for REAPI wire compression`

### Task 1.2 — Add `supported_wire_compressors` field to CapabilitiesConfig

**File:** `nativelink-config/src/cas_server.rs`

Add a new field to `CapabilitiesConfig` (around line 145):

```rust
#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct CapabilitiesConfig {
    /// Configuration for remote execution capabilities.
    /// If not set the capabilities service will inform the client that remote
    /// execution is not supported.
    pub remote_execution: Option<CapabilitiesRemoteExecutionConfig>,

    /// Wire compression algorithms supported for REAPI compressed-blobs.
    /// When set, the capabilities service will advertise these compressors
    /// and the ByteStream/CAS services will handle compressed data.
    ///
    /// Note: This is wire-level compression per the REAPI spec, which is
    /// orthogonal to at-rest compression (CompressionStore with LZ4).
    /// Wire compression reduces bytes on the network; at-rest compression
    /// reduces bytes in the storage backend.
    ///
    /// Bazel clients enable this with `--remote_cache_compression`.
    ///
    /// Default: empty (no wire compression, backwards compatible)
    #[serde(default)]
    pub supported_wire_compressors: Vec<WireCompressor>,
}
```

**Commit:** `feat(config): add supported_wire_compressors to CapabilitiesConfig`

### Task 1.3 — Wire config through to CapabilitiesServer

**File:** `nativelink-service/src/capabilities_server.rs`

1. Add a field to `CapabilitiesServer` to store the configured compressors:

```rust
#[derive(Debug, Default)]
pub struct CapabilitiesServer {
    supported_node_properties_for_instance: HashMap<InstanceName, Vec<String>>,
    supported_wire_compressors: Vec<compressor::Value>,
}
```

2. Add the import at the top:

```rust
use nativelink_proto::build::bazel::remote::execution::v2::compressor;
use nativelink_config::cas_server::WireCompressor;
```

3. In `CapabilitiesServer::new()`, collect the wire compressors from configs:

```rust
pub async fn new(
    configs: &[WithInstanceName<CapabilitiesConfig>],
    scheduler_map: &HashMap<String, Arc<dyn ClientStateManager>>,
) -> Result<Self, Error> {
    let mut supported_node_properties_for_instance = HashMap::new();
    let mut supported_wire_compressors = Vec::new();
    for config in configs {
        // ... existing property collection code ...
        for wc in &config.supported_wire_compressors {
            let proto_value = match wc {
                WireCompressor::Identity => compressor::Value::Identity,
                WireCompressor::Zstd => compressor::Value::Zstd,
            };
            if !supported_wire_compressors.contains(&proto_value) {
                supported_wire_compressors.push(proto_value);
            }
        }
    }
    Ok(Self {
        supported_node_properties_for_instance,
        supported_wire_compressors,
    })
}
```

4. In `get_capabilities()`, replace the hard-coded empty vectors:

```rust
supported_compressors: self.supported_wire_compressors.iter().map(|v| (*v as i32)).collect(),
supported_batch_update_compressors: self.supported_wire_compressors.iter().map(|v| (*v as i32)).collect(),
```

**Commit:** `feat(capabilities): advertise configured wire compressors in CacheCapabilities`

### Task 1.4 — Write test for CapabilitiesServer compressor advertisement

**File:** `nativelink-service/src/capabilities_server.rs` (add `#[cfg(test)]` module at bottom)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use nativelink_config::cas_server::WireCompressor;

    #[tokio::test]
    async fn test_no_wire_compressors_by_default() {
        let configs = vec![WithInstanceName {
            instance_name: "main".to_string(),
            config: CapabilitiesConfig {
                remote_execution: None,
                supported_wire_compressors: vec![],
            },
        }];
        let server = CapabilitiesServer::new(&configs, &HashMap::new())
            .await
            .unwrap();
        let resp = server
            .get_capabilities(Request::new(GetCapabilitiesRequest {
                instance_name: "main".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();

        let cache_caps = resp.cache_capabilities.unwrap();
        assert!(cache_caps.supported_compressors.is_empty());
        assert!(cache_caps.supported_batch_update_compressors.is_empty());
    }

    #[tokio::test]
    async fn test_zstd_wire_compressor_advertised() {
        let configs = vec![WithInstanceName {
            instance_name: "main".to_string(),
            config: CapabilitiesConfig {
                remote_execution: None,
                supported_wire_compressors: vec![WireCompressor::Zstd],
            },
        }];
        let server = CapabilitiesServer::new(&configs, &HashMap::new())
            .await
            .unwrap();
        let resp = server
            .get_capabilities(Request::new(GetCapabilitiesRequest {
                instance_name: "main".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();

        let cache_caps = resp.cache_capabilities.unwrap();
        assert_eq!(cache_caps.supported_compressors, vec![compressor::Value::Zstd as i32]);
        assert_eq!(cache_caps.supported_batch_update_compressors, vec![compressor::Value::Zstd as i32]);
    }
}
```

Run: `cargo test -p nativelink-service capabilities_server` 

**Commit:** `test(capabilities): verify wire compressor advertisement`

---

## Phase 2: Zstd Dependency and Wire Compression Utility

### Task 2.1 — Add zstd crate dependency to nativelink-service

**File:** `nativelink-service/Cargo.toml`

Add in the `[dependencies]` section:

```toml
zstd = { version = "0.13", default-features = false }
```

Also add to `[dev-dependencies]` for tests:

```toml
zstd = { version = "0.13", default-features = false }
```

**Commit:** `build(deps): add zstd crate for REAPI wire compression`

### Task 2.2 — Create wire compression utility module

**File:** `nativelink-service/src/wire_compression.rs` (NEW FILE)

```rust
// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use bytes::Bytes;
use nativelink_error::{Error, make_input_err};
use nativelink_proto::build::bazel::remote::execution::v2::compressor;

/// Compress raw bytes using the specified compressor.
/// Returns the compressed bytes, or an error if the compressor is unsupported.
pub fn compress(data: &[u8], compressor_value: compressor::Value) -> Result<Bytes, Error> {
    match compressor_value {
        compressor::Value::Identity => Ok(Bytes::copy_from_slice(data)),
        compressor::Value::Zstd => {
            let compressed = zstd::bulk::compress(data, 0)
                .map_err(|e| make_input_err!("Zstd compression failed: {}", e))?;
            Ok(Bytes::from(compressed))
        }
        _ => Err(make_input_err!(
            "Unsupported wire compressor: {:?}",
            compressor_value
        )),
    }
}

/// Decompress bytes using the specified compressor.
/// `expected_decompressed_size` is used to pre-allocate the output buffer for zstd.
/// Returns the decompressed bytes, or an error if the compressor is unsupported
/// or the data is corrupt.
pub fn decompress(
    data: &[u8],
    compressor_value: compressor::Value,
    expected_decompressed_size: usize,
) -> Result<Bytes, Error> {
    match compressor_value {
        compressor::Value::Identity => Ok(Bytes::copy_from_slice(data)),
        compressor::Value::Zstd => {
            let mut buf = Vec::with_capacity(expected_decompressed_size);
            zstd::stream::decode_all(data)
                .map_err(|e| make_input_err!("Zstd decompression failed: {}", e))?
                .into_iter()
                .for_each(|b| buf.push(b));
            Ok(Bytes::from(buf))
        }
        _ => Err(make_input_err!(
            "Unsupported wire compressor: {:?}",
            compressor_value
        )),
    }
}

/// Parse a compressor name string (from ResourceInfo) into a proto compressor::Value.
/// Returns None if the string is not a recognized compressor name.
pub fn compressor_from_str(name: &str) -> Option<compressor::Value> {
    match name {
        "identity" => Some(compressor::Value::Identity),
        "zstd" => Some(compressor::Value::Zstd),
        "deflate" => Some(compressor::Value::Deflate),
        "brotli" => Some(compressor::Value::Brotli),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_decompress_identity() {
        let data = b"hello world";
        let compressed = compress(data, compressor::Value::Identity).unwrap();
        assert_eq!(&compressed[..], data);
        let decompressed = decompress(&compressed, compressor::Value::Identity, data.len()).unwrap();
        assert_eq!(&decompressed[..], data);
    }

    #[test]
    fn test_compress_decompress_zstd() {
        let data = b"hello world, this is a test of zstd compression for REAPI wire format";
        let compressed = compress(data, compressor::Value::Zstd).unwrap();
        // zstd should compress this repetitive data to something smaller
        assert!(compressed.len() < data.len());
        let decompressed = decompress(&compressed, compressor::Value::Zstd, data.len()).unwrap();
        assert_eq!(&decompressed[..], data);
    }

    #[test]
    fn test_decompress_zstd_wrong_size_hint() {
        // Decompression should still work even with a wrong size hint,
        // because zstd is a streaming format that knows when to stop.
        let data = b"hello world";
        let compressed = compress(data, compressor::Value::Zstd).unwrap();
        let decompressed = decompress(&compressed, compressor::Value::Zstd, 9999).unwrap();
        assert_eq!(&decompressed[..], data);
    }

    #[test]
    fn test_compress_unsupported_returns_error() {
        let data = b"test";
        let result = compress(data, compressor::Value::Deflate);
        assert!(result.is_err());
    }

    #[test]
    fn test_decompress_unsupported_returns_error() {
        let data = b"test";
        let result = decompress(data, compressor::Value::Deflate, 4);
        assert!(result.is_err());
    }

    #[test]
    fn test_compressor_from_str() {
        assert_eq!(compressor_from_str("identity"), Some(compressor::Value::Identity));
        assert_eq!(compressor_from_str("zstd"), Some(compressor::Value::Zstd));
        assert_eq!(compressor_from_str("deflate"), Some(compressor::Value::Deflate));
        assert_eq!(compressor_from_str("brotli"), Some(compressor::Value::Brotli));
        assert_eq!(compressor_from_str("unknown"), None);
    }
}
```

**File:** `nativelink-service/src/lib.rs` — add `pub mod wire_compression;`

Run: `cargo test -p nativelink-service wire_compression`

**Commit:** `feat(service): add wire_compression utility module with zstd support`

### Task 2.3 — Fix decompress implementation to be more efficient

The `decompress` function in Task 2.2 uses a slow byte-by-byte approach. Replace it with:

```rust
pub fn decompress(
    data: &[u8],
    compressor_value: compressor::Value,
    expected_decompressed_size: usize,
) -> Result<Bytes, Error> {
    match compressor_value {
        compressor::Value::Identity => Ok(Bytes::copy_from_slice(data)),
        compressor::Value::Zstd => {
            let mut buf = Vec::with_capacity(expected_decompressed_size);
            let mut decoder = zstd::Decoder::new(data)
                .map_err(|e| make_input_err!("Failed to create zstd decoder: {}", e))?;
            std::io::Read::read_to_end(&mut decoder, &mut buf)
                .map_err(|e| make_input_err!("Zstd decompression failed: {}", e))?;
            Ok(Bytes::from(buf))
        }
        _ => Err(make_input_err!(
            "Unsupported wire compressor: {:?}",
            compressor_value
        )),
    }
}
```

Also add `use std::io::Read;` at top.

Run: `cargo test -p nativelink-service wire_compression`

**Commit:** `fix(service): use efficient zstd streaming decoder for decompression`

---

## Phase 3: ByteStream Server — Zstd Compression on Read/Write

### Task 3.1 — Add wire compressor awareness to ByteStreamServer InstanceInfo

**File:** `nativelink-service/src/bytestream_server.rs`

1. Add a `supported_wire_compressors` field to `InstanceInfo`:

```rust
pub struct InstanceInfo {
    store: Store,
    max_bytes_per_stream: usize,
    active_uploads: Arc<Mutex<HashMap<UuidKey, BytesWrittenAndIdleStream>>>,
    idle_stream_timeout: Duration,
    metrics: Arc<ByteStreamMetrics>,
    _sweeper_handle: Arc<JoinHandleDropGuard<()>>,
    /// Wire compressors this instance supports for compressed-blobs requests.
    supported_wire_compressors: Vec<compressor::Value>,
}
```

2. Add import: `use nativelink_proto::build::bazel::remote::execution::v2::compressor;`

3. Thread the config through in `ByteStreamServer::new()` — collect the wire compressors from the `CapabilitiesConfig` for each instance. The ByteStreamServer needs access to the capabilities configs. Add a new parameter to `ByteStreamServer::new()`:

```rust
pub fn new(
    configs: &[WithInstanceName<ByteStreamConfig>],
    capabilities_configs: &[WithInstanceName<CapabilitiesConfig>],
    store_manager: &StoreManager,
) -> Result<Self, Error> {
```

In the loop that builds `instance_infos`, look up the matching capabilities config for the instance name and extract its `supported_wire_compressors`:

```rust
let wire_compressors = capabilities_configs
    .iter()
    .find(|c| c.instance_name == config.instance_name)
    .map(|c| {
        c.supported_wire_compressors
            .iter()
            .map(|wc| match wc {
                WireCompressor::Identity => compressor::Value::Identity,
                WireCompressor::Zstd => compressor::Value::Zstd,
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
```

Then set `supported_wire_compressors: wire_compressors` when constructing `InstanceInfo`.

4. Update the `Debug` impl for `InstanceInfo` to include the new field.

5. Update all call sites of `ByteStreamServer::new()` to pass the capabilities configs. Search for `ByteStreamServer::new` in the workspace to find callers (likely in `nativelink-setup` or the main binary).

**Commit:** `feat(bytestream): add supported_wire_compressors to InstanceInfo`

### Task 3.2 — Handle compressed-blobs in ByteStream write (upload)

**File:** `nativelink-service/src/bytestream_server.rs`

In the `write()` method (around line 1075), after parsing `resource_info`, check if the client is sending compressed data and decompress before storing.

**Key insight:** For ByteStream write, when `resource_info.compressor` is Some("zstd"), the data arriving from the client is zstd-compressed. The server must decompress it before writing to the store. The `expected_size` in ResourceInfo is the UNCOMPRESSED size per REAPI spec.

There are two write paths to modify:

**Path 1: `inner_write_oneshot()`** (fast path, line ~830)

After collecting all data into `buffer`, before calling `store.update_oneshot()`:

```rust
// Decompress wire-compressed data before storing.
let final_data = if let Some(ref compressor_name) = stream.resource_info.compressor {
    let compressor_value = crate::wire_compression::compressor_from_str(compressor_name.as_ref())
        .ok_or_else(|| make_input_err!("Unknown compressor in resource name: {}", compressor_name))?;
    if compressor_value == compressor::Value::Identity {
        buffer.freeze()
    } else {
        // Verify the compressor is supported by this instance.
        if !instance_info.supported_wire_compressors.contains(&compressor_value) {
            return Err(make_input_err!(
                "Wire compressor {:?} not supported by this instance",
                compressor_value
            ));
        }
        let decompressed = crate::wire_compression::decompress(
            &buffer,
            compressor_value,
            stream.resource_info.expected_size,
        )?;
        // Verify decompressed size matches expected.
        if decompressed.len() != stream.resource_info.expected_size {
            return Err(make_input_err!(
                "Decompressed size {} does not match expected size {}",
                decompressed.len(),
                stream.resource_info.expected_size
            ));
        }
        decompressed
    }
} else {
    buffer.freeze()
};

let store = instance_info.store.clone();
store
    .update_oneshot(digest, final_data)
    .await
    .err_tip(|| "Error in update_oneshot")?;
```

**Path 2: `inner_write()`** (streaming path, line ~706)

This is more complex because data arrives in chunks through a channel. The approach: insert a decompression step between the client stream and the store's write channel.

The cleanest approach is to add a decompression wrapper around the `tx` (DropCloserWriteHalf). When `resource_info.compressor` is set to a non-identity value:

1. Create an intermediate channel pair
2. Spawn a task that reads compressed chunks from the intermediate channel, decompresses them, and writes to the real `tx`
3. Replace `tx` with the intermediate channel's write half

However, this is complex. A simpler approach for the streaming path: accumulate compressed data, then decompress before sending to store. But this defeats streaming.

**Simplest correct approach for streaming:** Buffer all compressed data in memory, then decompress and call `update_oneshot`. This means the streaming path falls back to oneshot behavior for compressed blobs. This is acceptable because:
- Compressed blobs are typically smaller than uncompressed (that's the point)
- The oneshot path already handles this well
- Bazel primarily uses ByteStream for large blobs, but compressed large blobs become smaller

**Implementation for `inner_write()`:** When compressor is non-identity, redirect to `inner_write_oneshot()` behavior (collect all data, decompress, update_oneshot).

Add a check at the top of the `write()` method, before choosing the write path:

```rust
let is_wire_compressed = stream
    .resource_info
    .compressor
    .as_ref()
    .is_some_and(|c| c.as_ref() != "identity");

// Compressed uploads must use the oneshot path because we need to
// decompress before storing, which requires the full data.
let force_oneshot = is_wire_compressed;
```

Then modify the `use_oneshot` logic to include `force_oneshot`:

```rust
let use_oneshot = force_oneshot || {
    // existing oneshot check logic
    if store.optimized_for(StoreOptimizations::SubscribesToUpdateOneshot)
        && expected_size <= 64 * 1024 * 1024
        && let Some(ref resource_uuid) = stream.resource_info.uuid
    {
        let is_single_shot = stream.is_first_msg_complete();
        if is_single_shot {
            let uuid_key = parse_uuid_to_key(resource_uuid);
            !instance.active_uploads.lock().contains_key(&uuid_key)
        } else {
            false
        }
    } else {
        false
    }
};
```

**Commit:** `feat(bytestream): decompress wire-compressed data on upload`

### Task 3.3 — Handle compressed-blobs in ByteStream read (download)

**File:** `nativelink-service/src/bytestream_server.rs`

In the `read()` method (around line 992), after parsing `resource_info`, check if the client is requesting compressed data and compress the response.

**Key insight:** For ByteStream read, when `resource_info.compressor` is Some("zstd"), the client wants the response data to be zstd-compressed. The server must read raw bytes from the store and compress them before sending.

The tricky part: `inner_read()` streams data in chunks. We need to compress the entire blob and then stream the compressed bytes back, OR compress chunk-by-chunk.

**Approach:** Read the entire blob from the store into memory, compress it, then stream the compressed bytes back. This is simpler and correct because:
- The client expects a single zstd frame for the whole blob
- Chunk-by-chunk compression would create multiple frames, which is not what the client expects
- The blob is already being read from the store; buffering it in memory is acceptable

Modify the `read()` method. After the GrpcStore shortcut and before calling `inner_read()`, add a compressed-read path:

```rust
// Check if client requested compressed data.
let requested_compressor = resource_info
    .compressor
    .as_ref()
    .and_then(|c| crate::wire_compression::compressor_from_str(c.as_ref()));

// Validate that the requested compressor is supported.
if let Some(ref comp) = requested_compressor {
    if *comp != compressor::Value::Identity
        && !instance.supported_wire_compressors.contains(comp)
    {
        return Err(Status::invalid_argument(format!(
            "Wire compressor {:?} not supported by this instance",
            comp
        )));
    }
}

// ... (existing digest_function, inner_read call) ...

// After getting the stream from inner_read, if compression is requested,
// read all data, compress, and return as a new stream.
let stream = match requested_compressor {
    Some(comp) if comp != compressor::Value::Identity => {
        // We need to collect the full data, compress, and re-stream.
        // This replaces the streaming response with a compressed one.
        let raw_stream = self
            .inner_read(instance, digest, read_request)
            .instrument(error_span!("bytestream_read_raw"))
            .with_context(
                make_ctx_for_hash_func(digest_function)
                    .err_tip(|| "In BytestreamServer::read")?,
            )
            .await
            .err_tip(|| "In ByteStreamServer::read")?;

        // Collect all data from the raw stream.
        let raw_data = collect_read_stream(raw_stream).await?;

        // Compress.
        let compressed_data = crate::wire_compression::compress(&raw_data, comp)
            .map_err(|e| Status::internal(e.to_string()))?;

        // Stream the compressed data back in chunks.
        stream_compressed_response(compressed_data, instance.max_bytes_per_stream)
    }
    _ => {
        // No compression or identity — use existing streaming path.
        self.inner_read(instance, digest, read_request)
            .instrument(error_span!("bytestream_read"))
            .with_context(
                make_ctx_for_hash_func(digest_function)
                    .err_tip(|| "In BytestreamServer::read")?,
            )
            .await
            .err_tip(|| "In ByteStreamServer::read")?
    }
};
```

Add helper functions:

```rust
/// Collects all data from a read stream into a single Bytes.
async fn collect_read_stream(
    mut stream: Pin<Box<dyn Stream<Item = Result<ReadResponse, Status>> + Send>>,
) -> Result<Bytes, Status> {
    let mut buf = BytesMut::new();
    while let Some(response) = stream.next().await {
        let response = response?;
        buf.extend_from_slice(&response.data);
    }
    Ok(buf.freeze())
}

/// Creates a stream of ReadResponse from compressed data, chunked by max_bytes_per_stream.
fn stream_compressed_response(
    compressed_data: Bytes,
    max_bytes_per_stream: usize,
) -> Pin<Box<dyn Stream<Item = Result<ReadResponse, Status>> + Send + 'static>> {
    let chunks = compressed_data
        .chunks(max_bytes_per_stream)
        .map(|chunk| {
            Ok(ReadResponse {
                data: Bytes::copy_from_slice(chunk),
            })
        })
        .collect::<Vec<_>>();
    Box::pin(futures::stream::iter(chunks))
}
```

**IMPORTANT:** The `expected_size` in the resource name for compressed-blobs is the UNCOMPRESSED size. But the `read_limit` and `read_offset` in the ReadRequest operate on the uncompressed data. We must apply them to the raw data before compressing.

**Revised approach:** Read from store with offset/limit applied, compress the result, then stream back. The offset/limit should be applied to the uncompressed data (which is what the store returns). This is already handled correctly because `inner_read()` applies the offset/limit to the store read.

**Commit:** `feat(bytestream): compress data on download for compressed-blobs requests`

### Task 3.4 — Update ByteStream write: validate compressor support and expected_size

**File:** `nativelink-service/src/bytestream_server.rs`

In the `write()` method, add validation early on:

```rust
// Validate wire compressor if specified in resource name.
let upload_compressor = stream
    .resource_info
    .compressor
    .as_ref()
    .and_then(|c| crate::wire_compression::compressor_from_str(c.as_ref()));

if let Some(ref comp) = upload_compressor {
    if *comp != compressor::Value::Identity
        && !instance.supported_wire_compressors.contains(comp)
    {
        return Err(Status::invalid_argument(format!(
            "Wire compressor {:?} not supported by this instance",
            comp
        )));
    }
}
```

**Commit:** `feat(bytestream): validate wire compressor on upload`

### Task 3.5 — Update ByteStreamServer::new() callers

**File:** Search for `ByteStreamServer::new` across the workspace. Likely in `nativelink-setup/src/lib.rs` or similar.

Add the `capabilities_configs` parameter to the call site. It should pass the same `capabilities` config slice that is used to construct `CapabilitiesServer`.

**Commit:** `feat(setup): wire capabilities config through to ByteStreamServer`

---

## Phase 4: CAS Server — BatchUpdateBlobs/BatchReadBlobs Compression

### Task 4.1 — Add wire compressor support to CasServer

**File:** `nativelink-service/src/cas_server.rs`

1. Add a `supported_wire_compressors` field to `CasServer`:

```rust
#[derive(Debug)]
pub struct CasServer {
    stores: HashMap<String, Store>,
    supported_wire_compressors: Vec<compressor::Value>,
}
```

2. Update `CasServer::new()` to accept capabilities configs:

```rust
pub fn new(
    configs: &[WithInstanceName<CasStoreConfig>],
    capabilities_configs: &[WithInstanceName<CapabilitiesConfig>],
    store_manager: &StoreManager,
) -> Result<Self, Error> {
    let mut stores = HashMap::with_capacity(configs.len());
    for config in configs {
        let store = store_manager.get_store(&config.cas_store).ok_or_else(|| {
            make_input_err!("'cas_store': '{}' does not exist", config.cas_store)
        })?;
        stores.insert(config.instance_name.clone(), store);
    }
    let supported_wire_compressors = capabilities_configs
        .iter()
        .flat_map(|c| {
            c.supported_wire_compressors.iter().map(|wc| match wc {
                WireCompressor::Identity => compressor::Value::Identity,
                WireCompressor::Zstd => compressor::Value::Zstd,
            })
        })
        .collect::<Vec<_>>();
    Ok(Self {
        stores,
        supported_wire_compressors,
    })
}
```

3. Update `CasServer::new()` callers to pass the extra parameter.

**Commit:** `feat(cas): add supported_wire_compressors to CasServer`

### Task 4.2 — Handle compression in BatchUpdateBlobs

**File:** `nativelink-service/src/cas_server.rs`

In `inner_batch_update_blobs()`, after extracting `request_data` from each blob request, decompress if the `compressor` field is set to a non-identity value:

```rust
// Inside the map closure in inner_batch_update_blobs:
let request_compressor = compressor::Value::try_from(request.compressor)
    .unwrap_or(compressor::Value::Identity);

// Validate that the compressor is supported.
if request_compressor != compressor::Value::Identity
    && !self.supported_wire_compressors.contains(&request_compressor)
{
    return Ok::<_, Error>(batch_update_blobs_response::Response {
        digest: Some(digest.clone()),
        status: Some(GrpcStatus {
            code: tonic::Code::InvalidArgument as i32,
            message: format!("Wire compressor {:?} not supported", request_compressor),
            ..Default::default()
        }),
    });
}

let request_data = if request_compressor == compressor::Value::Identity {
    request.data
} else {
    let decompressed = crate::wire_compression::decompress(
        &request.data,
        request_compressor,
        usize::try_from(digest_info.size_bytes())
            .err_tip(|| "Digest size_bytes was not convertible to usize")?,
    )?;
    decompressed
};

// Now use request_data (decompressed) for the size check and store write.
let size_bytes = usize::try_from(digest_info.size_bytes())
    .err_tip(|| "Digest size_bytes was not convertible to usize")?;
error_if!(
    size_bytes != request_data.len(),
    "Digest for upload had mismatching sizes, digest said {} data said {}",
    size_bytes,
    request_data.len()
);
```

**Commit:** `feat(cas): decompress wire-compressed data in BatchUpdateBlobs`

### Task 4.3 — Handle compression in BatchReadBlobs

**File:** `nativelink-service/src/cas_server.rs`

In `inner_batch_read_blobs()`, after reading the raw data from the store, check if the client sent `acceptable_compressors` and compress if a matching compressor is supported:

```rust
// After reading data from store, decide on response compressor.
let response_compressor = if request.acceptable_compressors.is_empty() {
    compressor::Value::Identity
} else {
    // Find the first acceptable compressor that we support.
    request
        .acceptable_compressors
        .iter()
        .filter_map(|&c| compressor::Value::try_from(c).ok())
        .find(|c| self.supported_wire_compressors.contains(c))
        .unwrap_or(compressor::Value::Identity)
};

let (final_data, final_compressor) = if response_compressor == compressor::Value::Identity {
    (data, compressor::Value::Identity)
} else {
    match crate::wire_compression::compress(&data, response_compressor) {
        Ok(compressed) => (compressed, response_compressor),
        Err(_) => (data, compressor::Value::Identity), // Fall back to identity on error
    }
};

Ok::<_, Error>(batch_read_blobs_response::Response {
    status: Some(status),
    digest: Some(digest),
    compressor: final_compressor.into(),
    data: final_data,
})
```

Replace the current hard-coded `compressor: compressor::Value::Identity.into()` (line 228) with the computed `final_compressor.into()`.

**Commit:** `feat(cas): compress response data in BatchReadBlobs when client accepts`

---

## Phase 5: Integration and End-to-End Testing

### Task 5.1 — Add integration test helper: compressed bytestream round-trip

**File:** `nativelink-service/src/bytestream_server.rs` (add to `#[cfg(test)]` module)

This test verifies the full round-trip: upload compressed -> download compressed.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use nativelink_config::cas_server::WireCompressor;
    use nativelink_store::memory_store::MemoryStore;
    use nativelink_util::common::DigestInfo;
    use nativelink_proto::google::bytestream::{ReadRequest, WriteRequest, WriteResponse};
    use nativelink_proto::build::bazel::remote::execution::v2::compressor;

    /// Helper to create a ByteStreamServer with zstd compression enabled.
    async fn make_test_server() -> ByteStreamServer {
        // Create an in-memory store.
        let store_manager = StoreManager::new();
        // ... set up store, configs, etc.
        // This will need to be fleshed out based on how other tests
        // in the codebase construct servers.
        todo!("Implement based on existing test patterns")
    }

    #[tokio::test]
    async fn test_bytestream_write_zstd_decompresses_before_store() {
        // 1. Create a server with zstd wire compression enabled.
        // 2. Write compressed-blobs data (zstd-compressed) via ByteStream.
        // 3. Read the data back as identity (plain blobs).
        // 4. Verify the raw data matches what we compressed.
    }

    #[tokio::test]
    async fn test_bytestream_read_zstd_compresses_response() {
        // 1. Create a server with zstd wire compression enabled.
        // 2. Write raw data via ByteStream (identity, plain blobs).
        // 3. Read the data back with compressed-blobs/zstd resource name.
        // 4. Verify the response is valid zstd that decompresses to the original.
    }

    #[tokio::test]
    async fn test_bytestream_rejects_unsupported_compressor() {
        // 1. Create a server with no wire compression enabled.
        // 2. Attempt to write with compressed-blobs/zstd resource name.
        // 3. Verify we get an InvalidArgument error.
    }
}
```

Note: The exact test structure depends on how the existing test infrastructure works in this project. Look at existing test patterns in `nativelink-store` or integration test crates for the helper setup pattern.

**Commit:** `test(bytestream): add integration tests for zstd wire compression`

### Task 5.2 — Add CAS server round-trip test for batch operations

**File:** `nativelink-service/src/cas_server.rs` (add to `#[cfg(test)]` module)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use nativelink_config::cas_server::WireCompressor;
    use nativelink_proto::build::bazel::remote::execution::v2::compressor;

    #[tokio::test]
    async fn test_batch_update_blobs_zstd_decompresses() {
        // 1. Create CAS server with zstd enabled.
        // 2. Send BatchUpdateBlobs with zstd-compressed data.
        // 3. Read back with BatchReadBlobs (identity).
        // 4. Verify data matches.
    }

    #[tokio::test]
    async fn test_batch_read_blobs_zstd_compresses_response() {
        // 1. Create CAS server with zstd enabled.
        // 2. Store raw data.
        // 3. Read back with BatchReadBlobs with acceptable_compressors=[zstd].
        // 4. Verify response compressor is zstd and data decompresses correctly.
    }

    #[tokio::test]
    async fn test_batch_update_blobs_rejects_unsupported_compressor() {
        // 1. Create CAS server with NO wire compression.
        // 2. Send BatchUpdateBlobs with compressor=Zstd.
        // 3. Verify error response.
    }
}
```

**Commit:** `test(cas): add integration tests for batch wire compression`

### Task 5.3 — Add example configuration

**File:** Add an example config snippet to the documentation or examples directory.

```json5
{
  "services": {
    "capabilities": [{
      "instance_name": "main",
      "supported_wire_compressors": ["zstd"]
    }],
    "cas": [{
      "instance_name": "main",
      "cas_store": "cas_store"
    }],
    "bytestream": [{
      "instance_name": "main",
      "cas_store": "cas_store"
    }]
  }
}
```

**Commit:** `docs: add example config for wire compression`

---

## Phase 6: Verification and Cleanup

### Task 6.1 — Run full test suite

```bash
cargo test -p nativelink-service
cargo test -p nativelink-store
cargo test -p nativelink-config
cargo test -p nativelink-util -- resource_info
```

All existing tests must continue to pass. The default config (no `supported_wire_compressors`) must produce no behavior change.

### Task 6.2 — Verify existing identity tests still pass

The existing `cas_server.rs` test at line 228 hard-codes `compressor::Value::Identity.into()`. After our change, when `supported_wire_compressors` is empty, the BatchReadBlobs response should still be `Identity` (which it will be, because our `response_compressor` logic falls back to Identity when no acceptable compressors match).

### Task 6.3 — Manual integration test with Bazel

```bash
# Start NativeLink with wire compression config
# Build a target with compression enabled
bazel build //some:target --remote_cache_compression --remote_cache=grpc://localhost:8443
```

This should not fail with "unsupported compressor" errors.

### Task 6.4 — Clippy and formatting

```bash
cargo clippy -p nativelink-service -p nativelink-config
cargo fmt --check
```

**Commit:** `chore: clippy and fmt fixes for wire compression`

---

## Summary of Files Modified/Created

| File | Action | Description |
|------|--------|-------------|
| `nativelink-config/src/cas_server.rs` | MODIFY | Add `WireCompressor` enum, `supported_wire_compressors` field to `CapabilitiesConfig` |
| `nativelink-service/Cargo.toml` | MODIFY | Add `zstd` dependency |
| `nativelink-service/src/lib.rs` | MODIFY | Add `pub mod wire_compression;` |
| `nativelink-service/src/wire_compression.rs` | CREATE | Wire compression utility (compress, decompress, helpers) |
| `nativelink-service/src/capabilities_server.rs` | MODIFY | Thread config through, advertise compressors, add tests |
| `nativelink-service/src/bytestream_server.rs` | MODIFY | Add compressor to InstanceInfo, handle compressed-blobs read/write |
| `nativelink-service/src/cas_server.rs` | MODIFY | Handle compression in BatchUpdateBlobs/BatchReadBlobs |
| `nativelink-setup/src/lib.rs` (or similar) | MODIFY | Wire capabilities configs to ByteStreamServer and CasServer |

## Key Invariants

1. **Store always holds uncompressed data.** Wire compression is applied/removed at the service boundary.
2. **Default config = no behavior change.** If `supported_wire_compressors` is empty, everything works exactly as before.
3. **expected_size in compressed-blobs URIs = uncompressed size.** This is per the REAPI spec.
4. **Identity is always allowed.** Even if not in `supported_wire_compressors`, Identity (no compression) is always accepted.
5. **CompressionStore (LZ4) is NOT modified.** At-rest and wire compression are orthogonal.

## Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| zstd decompression of malicious data could use excessive memory | Cap decompressed size using `expected_size` from the digest; reject if decompressed data exceeds it |
| Streaming ByteStream read requires buffering full blob for compression | Acceptable for now; can optimize later with streaming zstd encoder |
| Breaking existing clients that don't expect compressed responses | Server only compresses if client requests it via `acceptable_compressors` (CAS) or `compressed-blobs` URI (ByteStream) |
| Capabilities config not aligned with ByteStream/CAS config | Share the same `CapabilitiesConfig` across all services for a given instance name |
