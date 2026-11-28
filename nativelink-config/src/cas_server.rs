// Copyright 2024 Trace Machina, Inc. All rights reserved.
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

use core::net::SocketAddr;
use std::collections::{HashMap, HashSet};

use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
#[cfg(feature = "dev-schema")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::schedulers::{ExperimentalSimpleSchedulerBackend, SchedulerSpec};
use crate::serde_utils::{
    convert_boolean_with_shellexpand, convert_data_size_with_shellexpand,
    convert_duration_with_shellexpand, convert_numeric_with_shellexpand,
    convert_optional_numeric_with_shellexpand, convert_optional_string_with_shellexpand,
    convert_string_with_shellexpand, convert_vec_string_with_shellexpand,
};
use crate::stores::{
    ClientTlsConfig, ConfigDigestHashFunction, StoreRefName, StoreSpec, StoreType,
};

/// Name of the scheduler. This type will be used when referencing a
/// scheduler in the `CasConfig::schedulers`'s map key.
pub type SchedulerRefName = String;

/// Used when the config references `instance_name` in the protocol.
pub type InstanceName = String;

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct WithInstanceName<T> {
    /// Used when the config references `instance_name` in the protocol.
    #[serde(default)]
    pub instance_name: InstanceName,
    #[serde(flatten)]
    pub config: T,
}

impl<T> core::ops::Deref for WithInstanceName<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct NamedConfig<Spec> {
    pub name: String,
    #[serde(flatten)]
    pub spec: Spec,
}

#[derive(Deserialize, Serialize, Debug, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum HttpCompressionAlgorithm {
    /// No compression.
    #[default]
    None,

    /// Zlib compression.
    Gzip,
}

/// Note: Compressing data in the cloud rarely has a benefit, since most
/// cloud providers have very high bandwidth backplanes. However, for
/// clients not inside the data center, it might be a good idea to
/// compress data to and from the cloud. This will however come at a high
/// CPU and performance cost. If you are making remote execution share the
/// same CAS/AC servers as client's remote cache, you can create multiple
/// services with different compression settings that are served on
/// different ports. Then configure the non-cloud clients to use one port
/// and cloud-clients to use another.
#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct HttpCompressionConfig {
    /// The compression algorithm that the server will use when sending
    /// responses to clients. Enabling this will likely save a lot of
    /// data transfer, but will consume a lot of CPU and add a lot of
    /// latency.
    /// see: <https://github.com/tracemachina/nativelink/issues/109>
    ///
    /// Default: `HttpCompressionAlgorithm::None`
    pub send_compression_algorithm: Option<HttpCompressionAlgorithm>,

    /// The compression algorithm that the server will accept from clients.
    /// The server will broadcast the supported compression algorithms to
    /// clients and the client will choose which compression algorithm to
    /// use. Enabling this will likely save a lot of data transfer, but
    /// will consume a lot of CPU and add a lot of latency.
    /// see: <https://github.com/tracemachina/nativelink/issues/109>
    ///
    /// Default: {no supported compression}
    pub accepted_compression_algorithms: Vec<HttpCompressionAlgorithm>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct AcStoreConfig {
    /// The store name referenced in the `stores` map in the main config.
    /// This store name referenced here may be reused multiple times.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub ac_store: StoreRefName,

    /// Whether the Action Cache store may be written to, this if set to false
    /// it is only possible to read from the Action Cache.
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct CasStoreConfig {
    /// The store name referenced in the `stores` map in the main config.
    /// This store name referenced here may be reused multiple times.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub cas_store: StoreRefName,

    /// Optional and experimental: enables the REAPI `SplitBlob`/`SpliceBlob`
    /// RPCs used by content-defined chunking clients (e.g. Bazel's
    /// `--experimental_remote_cache_chunking`). When set, the capabilities
    /// service advertises blob split/splice support and `FastCDC` 2020
    /// parameters for this instance. When `cas_store` is a grpc store the
    /// RPCs are forwarded to the backend (which must support chunking with
    /// matching parameters); otherwise they are served locally.
    ///
    /// See `nativelink-config/examples/chunking_cas.json5` for a complete
    /// configuration example.
    ///
    /// Default: not set — chunking RPCs are rejected, nothing is advertised,
    /// and behavior is identical to when this option did not exist.
    #[serde(default)]
    pub experimental_chunking: Option<CasChunkingConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct CasChunkingConfig {
    /// The store name referenced in the `stores` map in the main config used
    /// to persist blob-to-chunks layouts. Keys are the digests of the
    /// original blobs and values are serialized chunk layouts (which do not
    /// hash to those digests), so this store MUST NOT perform content digest
    /// verification and MUST NOT be the same store as `cas_store` — writing
    /// layouts into the CAS would overwrite blob content. Using the same
    /// store name as `cas_store` is rejected at startup.
    ///
    /// Required unless `cas_store` is a grpc store: for proxied instances
    /// the `SplitBlob`/`SpliceBlob` RPCs are forwarded to the backend, which
    /// owns the chunk layouts, and setting an `index_store` is rejected at
    /// startup.
    #[serde(default, deserialize_with = "convert_optional_string_with_shellexpand")]
    pub index_store: Option<StoreRefName>,

    /// The average chunk size in bytes advertised to clients through the
    /// `FastCDC` 2020 capability parameters and used for server-side
    /// chunking in `SplitBlob`. Clients derive the minimum and maximum
    /// chunk sizes from this value (avg / 4 and avg * 4). The value must
    /// be between 1 KiB and 1 MiB.
    ///
    /// Default: 524288 (512 KiB)
    #[serde(default)]
    pub avg_chunk_size_bytes: u64,

    /// Maximum number of chunks accepted in a `SpliceBlob` request or
    /// produced by on-demand chunking in `SplitBlob`. Blobs that would
    /// produce more chunks are served without chunking (`SplitBlob` returns
    /// `NOT_FOUND` and clients fall back to a regular download). This bounds
    /// the size of stored chunk layouts and of `SplitBlobResponse` messages
    /// (roughly 80-140 bytes per chunk). At the default average chunk size
    /// the default cap supports blobs up to ~25 GiB; note that values above
    /// ~50000 may produce responses that exceed default gRPC message size
    /// limits on clients.
    ///
    /// Default: 50000
    #[serde(default)]
    pub max_chunk_count: u64,
}

impl CasChunkingConfig {
    /// Default for `avg_chunk_size_bytes`, the value recommended by the
    /// REAPI spec for `FastCdc2020Params`.
    pub const DEFAULT_AVG_CHUNK_SIZE_BYTES: u64 = 512 * 1024;
    /// Bounds for `avg_chunk_size_bytes` mandated by the REAPI spec for
    /// `FastCdc2020Params`.
    pub const MIN_AVG_CHUNK_SIZE_BYTES: u64 = 1024;
    pub const MAX_AVG_CHUNK_SIZE_BYTES: u64 = 1024 * 1024;
    /// Default for `max_chunk_count`.
    pub const DEFAULT_MAX_CHUNK_COUNT: u64 = 50_000;

    /// Returns `avg_chunk_size_bytes` with the default applied.
    #[must_use]
    pub const fn resolved_avg_chunk_size_bytes(&self) -> u64 {
        if self.avg_chunk_size_bytes == 0 {
            Self::DEFAULT_AVG_CHUNK_SIZE_BYTES
        } else {
            self.avg_chunk_size_bytes
        }
    }

    /// Returns `max_chunk_count` with the default applied.
    #[must_use]
    pub const fn resolved_max_chunk_count(&self) -> u64 {
        if self.max_chunk_count == 0 {
            Self::DEFAULT_MAX_CHUNK_COUNT
        } else {
            self.max_chunk_count
        }
    }

    /// Returns `avg_chunk_size_bytes` with the default applied, or an error
    /// when the configured value is outside the REAPI-mandated bounds.
    pub fn validated_avg_chunk_size_bytes(&self) -> Result<u64, Error> {
        let avg_chunk_size_bytes = self.resolved_avg_chunk_size_bytes();
        if !(Self::MIN_AVG_CHUNK_SIZE_BYTES..=Self::MAX_AVG_CHUNK_SIZE_BYTES)
            .contains(&avg_chunk_size_bytes)
        {
            return Err(make_err!(
                Code::InvalidArgument,
                "'experimental_chunking.avg_chunk_size_bytes' is {avg_chunk_size_bytes}, must be between {} and {}",
                Self::MIN_AVG_CHUNK_SIZE_BYTES,
                Self::MAX_AVG_CHUNK_SIZE_BYTES
            ));
        }
        Ok(avg_chunk_size_bytes)
    }
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct CapabilitiesRemoteExecutionConfig {
    /// Scheduler used to configure the capabilities of remote execution.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub scheduler: SchedulerRefName,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct CapabilitiesConfig {
    /// Configuration for remote execution capabilities.
    /// If not set the capabilities service will inform the client that remote
    /// execution is not supported.
    pub remote_execution: Option<CapabilitiesRemoteExecutionConfig>,

    /// Whether this instance supports Bazel remote cache compression.
    /// When enabled, the capabilities service advertises zstd wire compression
    /// and the ByteStream/CAS services accept REAPI compressed-blobs/zstd data.
    ///
    /// Bazel clients enable this with `--remote_cache_compression`.
    #[serde(
        default,
        skip_serializing_if = "is_default",
        deserialize_with = "convert_boolean_with_shellexpand"
    )]
    pub remote_cache_compression: bool,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct ExecutionConfig {
    /// Keep identical in-flight Buck2 actions from different invocations on
    /// separate workers. Completed action-cache results remain shared. Pair
    /// this with single-use workers when capturing execution-container files.
    /// Other build tools and requests without Buck2 invocation metadata retain
    /// the default deduplication behavior. All schedulers serving the instance
    /// must use a runtime supporting this execution scope before enabling it.
    #[serde(default, deserialize_with = "convert_boolean_with_shellexpand")]
    pub experimental_buck2_invocation_isolation: bool,

    /// Seconds between re-sends of the operation's current state on an
    /// `Execute` or `WaitExecution` stream that has produced no stage
    /// change. The Remote Execution API permits duplicate updates, and the
    /// periodic message gives the client and every intermediary (an L7 load
    /// balancer, for example) proof the stream is alive. Without it the
    /// server is silent for as long as an action sits queued or executing,
    /// so a client holding a connection whose server side is gone waits
    /// until its own timeout rather than failing when the path died.
    ///
    /// Default: 0 (disabled, no heartbeats are sent)
    #[serde(
        default,
        deserialize_with = "convert_duration_with_shellexpand",
        skip_serializing_if = "is_default",
        alias = "execute_stream_heartbeat"
    )]
    pub execute_stream_heartbeat_s: usize,

    /// The store name referenced in the `stores` map in the main config.
    /// This store name referenced here may be reused multiple times.
    /// This value must be a CAS store reference.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub cas_store: StoreRefName,

    /// The scheduler name referenced in the `schedulers` map in the main config.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub scheduler: SchedulerRefName,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct FetchConfig {
    /// The store name referenced in the `stores` map in the main config.
    /// This store name referenced here may be reused multiple times.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub fetch_store: StoreRefName,

    /// Optional HTTP and HTTPS downloader. Omitted means lookup-only, with no origin access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloader: Option<FetchDownloaderConfig>,
}

const fn default_allow_unknown_fetch_qualifiers() -> bool {
    true
}

/// Opt-in origin retrieval for the Remote Asset Fetch service.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct FetchDownloaderConfig {
    /// CAS used by this instance's CAS and `ByteStream` services.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub cas_store: StoreRefName,

    /// Explicit HTTP forward proxy URL, optionally including credentials.
    /// Prefer `http_proxy_secret_file` for credentials. Setting both is invalid.
    /// The default HTTP client connects directly and ignores proxy environment variables.
    /// An explicit proxy adds a concurrent alternative; the first successful transfer wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_proxy: Option<String>,

    /// File containing the proxy URL. Whitespace is trimmed; read once at startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_proxy_secret_file: Option<String>,

    /// Hosts bypassing the explicit proxy, using `reqwest` matching rules.
    /// Only applies to the configured proxy; an empty list bypasses no hosts.
    /// Proxy environment variables are not consulted.
    #[serde(default)]
    pub no_proxy: Vec<String>,

    /// Reject downloads without a supported checksum. Default: false.
    /// Supported `checksum.sri` algorithms: sha256, sha384, sha512 and blake3,
    /// independent of the digest function the blob is stored under in CAS.
    #[serde(default)]
    pub require_checksum: bool,

    /// Accept unrecognized qualifiers as opaque cache-key metadata without applying
    /// their semantics. Set false to reject them. Default: true.
    /// `bazel.canonical_id` is recognized as cache identity metadata.
    #[serde(default = "default_allow_unknown_fetch_qualifiers")]
    pub allow_unknown_qualifiers: bool,

    /// Maximum downloaded object size, checked even without Content-Length.
    /// Zero means no size limit (the default).
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub max_size_bytes: u64,

    /// Maximum simultaneous download requests per instance, including CAS publication.
    /// Zero selects the default: 4. Each request can race two attempts when a proxy
    /// is configured. A positive `max_size_bytes` also bounds each attempt's spool.
    #[serde(default)]
    pub max_concurrent_downloads: usize,

    /// Total origin retrieval timeout across all candidate URIs, including CAS writes.
    /// Request timeouts are clamped to this limit. Zero selects the default: 300 seconds.
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub timeout_s: u64,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct PushConfig {
    /// The store name referenced in the `stores` map in the main config.
    /// This store name referenced here may be reused multiple times.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub push_store: StoreRefName,

    /// Whether the Action Cache store may be written to, this if set to false
    /// it is only possible to read from the Action Cache.
    #[serde(default)]
    pub read_only: bool,
}

// From https://github.com/serde-rs/serde/issues/818#issuecomment-287438544
fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == Default::default()
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct ByteStreamConfig {
    /// Name of the store in the "stores" configuration.
    pub cas_store: StoreRefName,

    /// Max number of bytes to send on each grpc stream chunk.
    /// According to <https://github.com/grpc/grpc.github.io/issues/371>
    /// 16KiB - 64KiB is optimal.
    ///
    ///
    /// Default: 64KiB
    #[serde(
        default,
        deserialize_with = "convert_data_size_with_shellexpand",
        skip_serializing_if = "is_default"
    )]
    pub max_bytes_per_stream: usize,

    /// In the event a client disconnects while uploading a blob, we will hold
    /// the internal stream open for this many seconds before closing it.
    /// This allows clients that disconnect to reconnect and continue uploading
    /// the same blob.
    ///
    /// Default: 10 seconds
    #[serde(
        default,
        deserialize_with = "convert_duration_with_shellexpand",
        skip_serializing_if = "is_default",
        alias = "persist_stream_on_disconnect_timeout"
    )]
    pub persist_stream_on_disconnect_timeout_s: usize,

    /// Recover a resumed upload (a write whose first offset is nonzero) that
    /// reaches an instance holding no state for its UUID, which happens when
    /// several instances sit behind a load balancer without session affinity.
    /// When the blob already exists in the store, the write completes
    /// immediately; otherwise the client receives UNAVAILABLE, re-queries the
    /// write status (an instance holding the upload answers with its offset,
    /// any other answers 0) and resumes or restarts from there. With this
    /// off, such a resume fails the upload with a non-retryable
    /// "out of order data" error.
    ///
    /// Default: true
    #[serde(default = "default_true")]
    pub recover_unknown_uuid_resumes: bool,
}

// Not derived: the derive would default `recover_unknown_uuid_resumes` to
// false, diverging from the deserialization default above.
impl Default for ByteStreamConfig {
    fn default() -> Self {
        Self {
            cas_store: StoreRefName::default(),
            max_bytes_per_stream: 0,
            persist_stream_on_disconnect_timeout_s: 0,
            recover_unknown_uuid_resumes: true,
        }
    }
}

// Older bytestream config. All fields are as per the newer docs, but this requires
// the hashed `cas_stores` v.s. the WithInstanceName approach. This should _not_ be updated
// with newer fields, and eventually dropped
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct OldByteStreamConfig {
    pub cas_stores: HashMap<InstanceName, StoreRefName>,
    #[serde(
        default,
        deserialize_with = "convert_data_size_with_shellexpand",
        skip_serializing_if = "is_default"
    )]
    pub max_bytes_per_stream: usize,
    #[serde(
        default,
        deserialize_with = "convert_data_size_with_shellexpand",
        skip_serializing_if = "is_default"
    )]
    pub max_decoding_message_size: usize,
    #[serde(
        default,
        deserialize_with = "convert_duration_with_shellexpand",
        skip_serializing_if = "is_default",
        alias = "persist_stream_on_disconnect_timeout"
    )]
    pub persist_stream_on_disconnect_timeout_s: usize,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct WorkerApiConfig {
    /// The scheduler name referenced in the `schedulers` map in the main config.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub scheduler: SchedulerRefName,

    /// Disable the periodic sweep that tells workers to kill operations
    /// the scheduler no longer has executing on them (for example because
    /// every client disconnected before the action finished). With the
    /// sweep disabled such orphaned actions run to completion and still
    /// warm the action cache, which can be preferable for deployments
    /// with long or expensive actions whose clients merely retry.
    ///
    /// Default: false (the sweep runs)
    #[serde(default)]
    pub disable_kill_revoked_operations: bool,

    /// How often, in seconds, the scheduler checks for operations that
    /// are still running on a worker but are no longer executing
    /// according to the state manager, and tells the worker to kill
    /// them. Each pass costs one state-manager lookup per running
    /// operation, so store-backed deployments with many concurrent
    /// actions may want a longer interval.
    ///
    /// Default: 5 (0 uses the default)
    #[serde(default)]
    pub kill_revoked_operations_interval_s: u64,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct AdminConfig {
    /// Path to register the admin API. If path is "/admin", and your
    /// domain is "example.com", you can reach the endpoint with:
    /// <http://example.com/admin>.
    ///
    /// Default: "/admin"
    #[serde(default)]
    pub path: String,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct HealthConfig {
    /// Path to register the health status check. If path is "/status", and your
    /// domain is "example.com", you can reach the endpoint with:
    /// <http://example.com/status>.
    ///
    /// Default: "/status"
    #[serde(default)]
    pub path: String,

    /// Timeout on health checks. Default: 5s.
    #[serde(default)]
    pub timeout_seconds: u64,

    /// Path of the readiness check, the stricter sibling of `path`: it
    /// answers 503 while any component is still initializing, where `path`
    /// answers 200. A worker's registration with its scheduler is such a
    /// component, so a Kubernetes readiness probe on this path turns Ready
    /// only once the worker can take work. Must differ from `path`; the
    /// same path for both is refused at startup.
    ///
    /// Default: "/ready"
    #[serde(default)]
    pub readiness_path: String,
}

#[derive(Deserialize, Serialize, Debug)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct BepConfig {
    /// The store to publish build events to.
    /// The store name referenced in the `stores` map in the main config.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub store: StoreRefName,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct IdentityHeaderSpec {
    /// The name of the header to look for the identity in.
    /// Default: "x-identity"
    #[serde(default, deserialize_with = "convert_optional_string_with_shellexpand")]
    pub header_name: Option<String>,

    /// If the header is required to be set or fail the request.
    #[serde(default)]
    pub required: bool,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct OriginEventsPublisherSpec {
    /// The store to publish nativelink events to.
    /// The store name referenced in the `stores` map in the main config.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub store: StoreRefName,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct OriginEventsSpec {
    /// The publisher configuration for origin events.
    pub publisher: OriginEventsPublisherSpec,

    /// The maximum number of events to queue before applying back pressure.
    /// IMPORTANT: Backpressure causes all clients to slow down significantly.
    /// Zero is default.
    ///
    /// Default: 65536 (zero defaults to this)
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_event_queue_size: usize,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct ServicesConfig {
    /// The Content Addressable Storage (CAS) backend config.
    /// The key is the `instance_name` used in the protocol and the
    /// value is the underlying CAS store config.
    #[serde(
        default,
        deserialize_with = "super::backcompat::opt_vec_with_instance_name"
    )]
    pub cas: Option<Vec<WithInstanceName<CasStoreConfig>>>,

    /// The Action Cache (AC) backend config.
    /// The key is the `instance_name` used in the protocol and the
    /// value is the underlying AC store config.
    #[serde(
        default,
        deserialize_with = "super::backcompat::opt_vec_with_instance_name"
    )]
    pub ac: Option<Vec<WithInstanceName<AcStoreConfig>>>,

    /// Capabilities service is required in order to use most of the
    /// bazel protocol. This service is used to provide the supported
    /// features and versions of this bazel gRPC service.
    #[serde(
        default,
        deserialize_with = "super::backcompat::opt_vec_with_instance_name"
    )]
    pub capabilities: Option<Vec<WithInstanceName<CapabilitiesConfig>>>,

    /// The remote execution service configuration.
    /// NOTE: This service is under development and is currently just a
    /// place holder.
    #[serde(
        default,
        deserialize_with = "super::backcompat::opt_vec_with_instance_name"
    )]
    pub execution: Option<Vec<WithInstanceName<ExecutionConfig>>>,

    /// This is the service used to stream data to and from the CAS.
    /// Bazel's protocol strongly encourages users to use this streaming
    /// interface to interact with the CAS when the data is large.
    #[serde(default, deserialize_with = "super::backcompat::opt_bytestream")]
    pub bytestream: Option<Vec<WithInstanceName<ByteStreamConfig>>>,

    /// These two are collectively the Remote Asset protocol, but it's
    /// defined as two separate services
    #[serde(
        default,
        deserialize_with = "super::backcompat::opt_vec_with_instance_name"
    )]
    pub fetch: Option<Vec<WithInstanceName<FetchConfig>>>,

    #[serde(
        default,
        deserialize_with = "super::backcompat::opt_vec_with_instance_name"
    )]
    pub push: Option<Vec<WithInstanceName<PushConfig>>>,

    /// This is the service used for workers to connect and communicate
    /// through.
    /// NOTE: This service should be served on a different, non-public port.
    /// In other words, `worker_api` configuration should not have any other
    /// services that are served on the same port. Doing so is a security
    /// risk, as workers have a different permission set than a client
    /// that makes the remote execution/cache requests.
    pub worker_api: Option<WorkerApiConfig>,

    /// Experimental - Build Event Protocol (BEP) configuration. This is
    /// the service that will consume build events from the client and
    /// publish them to a store for processing by an external service.
    pub experimental_bep: Option<BepConfig>,

    /// This is the service for any administrative tasks.
    /// It provides a REST API endpoint for administrative purposes.
    pub admin: Option<AdminConfig>,

    /// This is the service for health status check.
    pub health: Option<HealthConfig>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct TlsConfig {
    /// Path to the certificate file.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub cert_file: String,

    /// Path to the private key file.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub key_file: String,

    /// Path to the certificate authority for mTLS, if client authentication is
    /// required for this endpoint.
    #[serde(default, deserialize_with = "convert_optional_string_with_shellexpand")]
    pub client_ca_file: Option<String>,

    /// Path to the certificate revocation list for mTLS, if client
    /// authentication is required for this endpoint.
    #[serde(default, deserialize_with = "convert_optional_string_with_shellexpand")]
    pub client_crl_file: Option<String>,
}

/// Advanced Http configurations. These are generally should not be set.
/// For documentation on what each of these do, see the hyper documentation:
/// See: <https://docs.rs/hyper/latest/hyper/server/conn/struct.Http.html>
///
/// Note: All of these default to hyper's default values unless otherwise
/// specified.
#[derive(Deserialize, Serialize, Debug, Default, Clone, Copy)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct HttpServerConfig {
    /// Interval to send keep-alive pings via HTTP2.
    /// Note: This is in seconds.
    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    pub http2_keep_alive_interval: Option<u32>,

    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    pub experimental_http2_max_pending_accept_reset_streams: Option<u32>,

    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    pub experimental_http2_initial_stream_window_size: Option<u32>,

    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    pub experimental_http2_initial_connection_window_size: Option<u32>,

    #[serde(default)]
    pub experimental_http2_adaptive_window: Option<bool>,

    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    pub experimental_http2_max_frame_size: Option<u32>,

    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    pub experimental_http2_max_concurrent_streams: Option<u32>,

    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand",
        alias = "experimental_http2_keep_alive_timeout"
    )]
    pub experimental_http2_keep_alive_timeout_s: Option<u32>,

    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    pub experimental_http2_max_send_buf_size: Option<u32>,

    #[serde(default)]
    pub experimental_http2_enable_connect_protocol: Option<bool>,

    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    pub experimental_http2_max_header_list_size: Option<u32>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum ListenerConfig {
    /// Listener for HTTP/HTTPS/HTTP2 sockets.
    Http(HttpListener),
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct HttpListener {
    /// Address to listen on. Example: `127.0.0.1:8080` or `:8080` to listen
    /// to all IPs.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub socket_address: String,

    /// Allow binding `socket_address` before it is assigned locally.
    ///
    /// Default: false
    #[serde(default)]
    pub freebind: bool,

    /// Disable batching of small TCP writes on accepted connections, including
    /// HTTP/2 control frames.
    /// Applies to both plaintext and TLS connections. Set to true to opt into
    /// sending small writes promptly. When false or omitted, preserves default
    /// TCP buffering behavior.
    /// Older binaries without this option reject the field even when set to
    /// false. Config templates must omit it until all consuming binaries support
    /// it. Remove the field entirely before rolling back to an older binary.
    ///
    /// Default: false
    #[serde(default)]
    pub tcp_nodelay: bool,

    /// Data transport compression configuration to use for this service.
    #[serde(default)]
    pub compression: HttpCompressionConfig,

    /// Advanced Http server configuration.
    #[serde(default)]
    pub advanced_http: HttpServerConfig,

    /// Maximum number of bytes to decode on each grpc stream chunk.
    /// Default: 4 MiB
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub max_decoding_message_size: usize,

    /// Tls Configuration for this server.
    /// If not set, the server will not use TLS.
    ///
    /// Default: None
    #[serde(default)]
    pub tls: Option<TlsConfig>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct ServerConfig {
    /// Name of the server. This is used to help identify the service
    /// for telemetry and logs.
    ///
    /// Default: {index of server in config}
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub name: String,

    /// Configuration
    pub listener: ListenerConfig,

    /// Services to attach to server.
    pub services: Option<ServicesConfig>,

    /// The config related to identifying the client.
    /// Default: {see `IdentityHeaderSpec`}
    #[serde(default)]
    pub experimental_identity_header: IdentityHeaderSpec,
}

/// The scale a CPU property is advertised on.
#[derive(Copy, Clone, Deserialize, Serialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum CpuUnit {
    /// Whole cores, rounded down: what `cpu_count: { query_cmd: "nproc" }`
    /// advertises and what an action asking for `cpu_count=1` means.
    #[default]
    Cores,
    /// Thousandths of a core, for a fleet whose actions and workers already
    /// speak that scale.
    Millicores,
}

/// Advertise CPU and memory from what the worker can actually see. The
/// worker reads `cpu.max` and `memory.max` from its own cgroup v2 directory
/// (found through `/proc/self/cgroup`, so a privileged container that sees
/// the host's tree still finds its own) up to the nearest limited ancestor,
/// the pod's limits on Kubernetes; with no limit at any level the configured
/// properties stand. It takes off what it needs for itself, divides
/// the memory by the enforcement headroom, and sets the two properties to
/// the result at registration. What the scheduler packs against is then
/// derived from the one number that is enforced, instead of typed in twice.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct CapacityConfig {
    /// Property that carries CPU, on the scale `cpu_unit` names.
    /// Default: `cpu_count`
    #[serde(
        default = "default_capacity_cpu_property_name",
        deserialize_with = "convert_string_with_shellexpand"
    )]
    pub cpu_property_name: String,

    /// Property that carries memory, in KiB.
    /// Default: `memory_kb`
    #[serde(
        default = "default_capacity_memory_property_name",
        deserialize_with = "convert_string_with_shellexpand"
    )]
    pub memory_property_name: String,

    /// The scale the CPU property is advertised on. Whole cores by default,
    /// the scale every other example in this configuration uses; a fleet
    /// that advertises and requests thousandths of a core sets `millicores`.
    /// Getting this wrong is a thousandfold error in how many actions the
    /// scheduler packs onto the worker.
    /// Default: cores
    #[serde(default)]
    pub cpu_unit: CpuUnit,

    /// CPU the worker keeps for itself, in thousandths of a core whatever
    /// `cpu_unit` says.
    /// Default: 1000
    #[serde(
        default = "default_capacity_overhead_cpu_millicores",
        deserialize_with = "convert_numeric_with_shellexpand"
    )]
    pub overhead_cpu_millicores: u64,

    /// Memory the worker keeps for itself (the process, its directory cache,
    /// output buffers), in KiB.
    /// Default: 2097152 (2 GiB)
    #[serde(
        default = "default_capacity_overhead_memory_kb",
        deserialize_with = "convert_numeric_with_shellexpand"
    )]
    pub overhead_memory_kb: u64,

    /// Percent the advertised memory is reduced by, so that actions at their
    /// enforcement ceiling (reservation plus headroom) still fit the limit.
    /// Unset, it follows `resource_enforcement.memory_headroom_percent`
    /// while memory enforcement is on, and is 0 otherwise, so the two
    /// cannot drift apart by being typed twice.
    /// Default: unset
    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    pub memory_headroom_percent: Option<u64>,
}

fn default_capacity_cpu_property_name() -> String {
    "cpu_count".to_string()
}

fn default_capacity_memory_property_name() -> String {
    "memory_kb".to_string()
}

const fn default_capacity_overhead_cpu_millicores() -> u64 {
    1000
}

const fn default_capacity_overhead_memory_kb() -> u64 {
    2_097_152
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum WorkerProperty {
    /// List of static values.
    /// Note: Generally there should only ever be 1 value, but if the platform
    /// property key is `PropertyType::Priority` it may have more than one value.
    #[serde(deserialize_with = "convert_vec_string_with_shellexpand")]
    Values(Vec<String>),

    /// A dynamic configuration. The string will be executed as a command
    /// (not shell) and will be split by "\n" (new line character).
    QueryCmd(String),
}

/// Generic config for an endpoint and associated configs.
#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct EndpointConfig {
    /// URI of the endpoint.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub uri: String,

    /// Timeout in seconds for connecting to the endpoint and for a request
    /// to be answered. The worker's messages to the scheduler (keepalives,
    /// acknowledgements, results) ride on the one `ConnectWorker` stream
    /// opened at registration, which this does not bound; the `GoingAway`
    /// sent at shutdown is bounded by it, so a stalled connection cannot
    /// hold a pod past its grace period. A keepalive goes out every half of
    /// it.
    /// Default: 5 seconds
    pub timeout: Option<f32>,

    /// TCP keepalive interval (seconds), probing the socket under the
    /// scheduler connection so a dead peer is noticed at the OS level.
    /// If not set or 0, defaults to 30 seconds.
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub tcp_keepalive_s: u64,

    /// HTTP/2 keepalive interval (seconds): PING frames on the scheduler
    /// connection, so one that has gone dead underneath fails instead of
    /// hanging every message on it.
    /// If not set or 0, defaults to 30 seconds.
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub http2_keepalive_interval_s: u64,

    /// HTTP/2 keepalive timeout (seconds): a PING unanswered for this long
    /// ends the connection, and the worker reconnects.
    /// If not set or 0, defaults to 20 seconds.
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub http2_keepalive_timeout_s: u64,

    /// The TLS configuration to use to connect to the endpoint.
    pub tls_config: Option<ClientTlsConfig>,
}

#[derive(Copy, Clone, Deserialize, Serialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum MemoryEnforcement {
    /// Measure only.
    None,
    /// Kill the action's process group once two consecutive samples exceed
    /// the reservation plus headroom, or one sample exceeds twice it.
    #[default]
    Soft,
}

#[derive(Clone, Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct ResourceEnforcementConfig {
    /// What to do with an action that exceeds its memory reservation.
    /// Default: soft
    #[serde(default)]
    pub memory: MemoryEnforcement,

    /// Whether to refuse an action whose disk reservation exceeds the free
    /// space under the work directory, less what the actions already
    /// admitted reserved, before its inputs are fetched. The
    /// refusal is `ResourceExhausted`, which the scheduler requeues without
    /// counting an attempt and holds off this worker until its next
    /// keepalive.
    /// Default: none
    #[serde(default)]
    pub disk: DiskEnforcement,

    /// Platform property carrying the action's disk reservation in KiB.
    /// Default: `disk_kb`
    #[serde(
        default = "default_disk_property_name",
        deserialize_with = "convert_string_with_shellexpand"
    )]
    pub disk_property_name: String,

    /// Platform property carrying the action's memory reservation in KiB,
    /// as the scheduler sends it. An action without the property is not
    /// enforced.
    /// Default: `memory_kb`
    #[serde(
        default = "default_memory_property_name",
        deserialize_with = "convert_string_with_shellexpand"
    )]
    pub memory_property_name: String,

    /// Percent above the reservation an action may reach before it is
    /// killed.
    /// Default: 20
    #[serde(
        default = "default_memory_headroom_percent",
        deserialize_with = "convert_numeric_with_shellexpand"
    )]
    pub memory_headroom_percent: u64,

    /// Percent above its disk reservation an action's own files may reach
    /// before it is killed, with `disk: soft`.
    /// Default: 20
    #[serde(
        default = "default_memory_headroom_percent",
        deserialize_with = "convert_numeric_with_shellexpand"
    )]
    pub disk_headroom_percent: u64,

    /// Kill one running action once the worker's cgroup has spent at least
    /// this share of the last ten seconds, as a whole number from 0 to 100,
    /// with every task stalled on memory (`full avg10` in cgroup v2
    /// `memory.pressure`) for `memory_pressure_kill_after_s` without a
    /// break. A worker whose actions fit under its limit only by thrashing
    /// never reaches the OOM killer and stops answering the scheduler; this
    /// ends the action instead, as a memory kill, so the scheduler requeues
    /// it with a larger reservation. The kill lands on the action furthest
    /// over its reservation, or the largest when none is over, and never
    /// on one that reserves the whole worker. It does not check that the
    /// actions caused the pressure: pressure from the worker itself (page
    /// cache from the filesystem store, a large upload) still kills an
    /// action. Needs `memory: soft`, which supplies the per-action samples
    /// the choice is made from. Values over 100 are read as 100. 0 disables.
    /// Default: 0
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub memory_pressure_kill_percent: u64,

    /// How long `memory.pressure` must stay at or above
    /// `memory_pressure_kill_percent` before an action is killed, in
    /// seconds. Another kill waits this long again. 0 is read as 1.
    /// Default: 30
    #[serde(
        default = "default_memory_pressure_kill_after_s",
        deserialize_with = "convert_duration_with_shellexpand"
    )]
    pub memory_pressure_kill_after_s: u64,
}

const fn default_memory_pressure_kill_after_s() -> u64 {
    30
}

fn default_memory_property_name() -> String {
    "memory_kb".to_string()
}

const fn default_true() -> bool {
    true
}

fn default_disk_property_name() -> String {
    "disk_kb".to_string()
}

#[derive(Copy, Clone, Deserialize, Serialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum DiskEnforcement {
    /// Fetch inputs regardless of free space.
    #[default]
    None,
    /// Refuse the action when the free space under the work directory is
    /// below its reservation.
    Guard,
    /// `guard`, and kill an action whose files under its own directory
    /// grow past its reservation plus `disk_headroom_percent`. Sampled
    /// every ten seconds by walking the action directory, counting only
    /// files the action itself wrote (inputs are hard links into the CAS
    /// and are not counted).
    Soft,
}

const fn default_memory_headroom_percent() -> u64 {
    20
}

#[derive(Copy, Clone, Deserialize, Serialize, Debug, Default)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum UploadCacheResultsStrategy {
    /// Only upload action results with an exit code of 0.
    #[default]
    SuccessOnly,

    /// Don't upload any action results.
    Never,

    /// Upload all action results that complete.
    Everything,

    /// Only upload action results that fail.
    FailuresOnly,
}

#[derive(Clone, Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum EnvironmentSource {
    /// The name of the platform property in the action to get the value from.
    Property(String),

    /// The raw value to set.
    Value(#[serde(deserialize_with = "convert_string_with_shellexpand")] String),

    /// Take the value from the local environment corresponding to the name key
    FromEnvironment,

    /// The max amount of time in milliseconds the command is allowed to run
    /// (requested by the client).
    TimeoutMillis,

    /// A special file path will be provided that can be used to communicate
    /// with the parent process about out-of-band information. This file
    /// will be read after the command has finished executing. Based on the
    /// contents of the file, the behavior of the result may be modified.
    ///
    /// The format of the file contents should be json with the following
    /// schema:
    /// {
    ///   // If set the command will be considered a failure.
    ///   // May be one of the following static strings:
    ///   // "timeout": Will Consider this task to be a timeout.
    ///   "failure": "timeout",
    /// }
    ///
    /// The file may also carry `peak_gpu_memory_kb`, a number: the peak
    /// device memory the action held on its GPU, in KiB, when whatever
    /// wraps the action measured it. The worker reads no device itself; it
    /// reports the figure with the action's other resource usage, and a
    /// scheduler that learns from its workers sizes the action by it.
    ///
    /// All fields are optional, file does not need to be created and may be
    /// empty.
    SideChannelFile,

    /// A "root" directory for the action. This directory can be used to
    /// store temporary files that are not needed after the action has
    /// completed. This directory will be purged after the action has
    /// completed.
    ///
    /// For example:
    /// If an action writes temporary data to a path but nativelink should
    /// clean up this path after the job has executed, you may create any
    /// directory under the path provided in this variable. A common pattern
    /// would be to use `entrypoint` to set a shell script that reads this
    /// variable, `mkdir $ENV_VAR_NAME/tmp` and `export TMPDIR=$ENV_VAR_NAME/tmp`.
    /// Another example might be to bind-mount the `/tmp` path in a container to
    /// this path in `entrypoint`.
    ActionDirectory,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct UploadActionResultConfig {
    /// Underlying AC store that the worker will use to publish execution results
    /// into. Objects placed in this store should be reachable from the
    /// scheduler/client-cas after they have finished updating.
    /// Default: {No uploading is done}
    pub ac_store: Option<StoreRefName>,

    /// In which situations should the results be published to the `ac_store`,
    /// if set to `SuccessOnly` then only results with an exit code of 0 will be
    /// uploaded, if set to Everything all completed results will be uploaded.
    ///
    /// Default: `SuccessOnly`
    #[serde(default)]
    pub upload_ac_results_strategy: UploadCacheResultsStrategy,

    /// Store to upload historical results to. This should be a CAS store if set.
    ///
    /// Default: {CAS store of parent}
    pub historical_results_store: Option<StoreRefName>,

    /// In which situations should the results be published to the historical CAS.
    /// The historical CAS is where failures are published. These messages conform
    /// to the CAS key-value lookup format and are always a `HistoricalExecuteResponse`
    /// serialized message.
    ///
    /// Default: `FailuresOnly`
    #[serde(default)]
    pub upload_historical_results_strategy: Option<UploadCacheResultsStrategy>,

    /// Template to use for the `ExecuteResponse.message` property. This message
    /// is attached to the response before it is sent to the client. The following
    /// special variables are supported:
    /// - `digest_function`: Digest function used to calculate the action digest.
    /// - `action_digest_hash`: Action digest hash.
    /// - `action_digest_size`: Action digest size.
    /// - `historical_results_hash`: `HistoricalExecuteResponse` digest hash.
    /// - `historical_results_size`: `HistoricalExecuteResponse` digest size.
    ///
    /// A common use case of this is to provide a link to the web page that
    /// contains more useful information for the user.
    ///
    /// An example that is fully compatible with `bb_browser` is:
    /// <https://example.com/my-instance-name-here/blobs/{digest_function}/action/{action_digest_hash}-{action_digest_size}/>
    ///
    /// Default: "" (no message)
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub success_message_template: String,

    /// Same as `success_message_template` but for failure case.
    ///
    /// An example that is fully compatible with `bb_browser` is:
    /// <https://example.com/my-instance-name-here/blobs/{digest_function}/historical_execute_response/{historical_results_hash}-{historical_results_size}/>
    ///
    /// Default: "" (no message)
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub failure_message_template: String,
}

/// The pool of persistent worker processes (Bazel `supports-workers`).
#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct PersistentWorkersConfig {
    /// Run an action whose platform properties carry `supports-workers=1`
    /// in a pooled worker process that outlives it and serves the next
    /// action with the same executable, startup arguments and environment.
    /// Off, every such action runs as a one-shot process like any other.
    /// Default: true
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Most worker processes kept per key (executable, startup arguments,
    /// environment, protocol). An action whose key is at the cap and has
    /// no idle process waits `acquire_timeout_s` for one to come back, then
    /// runs one-shot. 0 takes the default.
    /// Default: 4
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_workers_per_key: usize,

    /// Seconds an idle worker process is kept before the sweeper shuts it
    /// down. 0 takes the default.
    /// Default: 300
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub idle_timeout_s: u64,

    /// Requests a worker process serves before it is retired and replaced,
    /// which bounds what a long-lived process accumulates. 0 takes the
    /// default.
    /// Default: 200
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_requests_per_worker: u64,

    /// Milliseconds a worker process being shut down gets to exit on its
    /// own (its stdin is closed) before SIGKILL. 0 takes the default.
    /// Default: 5000
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub shutdown_grace_ms: u64,

    /// Seconds an action waits for a worker process to come back when its
    /// key is at `max_workers_per_key` with none idle, before it runs
    /// one-shot instead. 0 takes the default.
    /// Default: 30
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub acquire_timeout_s: u64,
}

/// The same as an absent block: the pool on, every size at its default.
impl Default for PersistentWorkersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_workers_per_key: 0,
            idle_timeout_s: 0,
            max_requests_per_worker: 0,
            shutdown_grace_ms: 0,
            acquire_timeout_s: 0,
        }
    }
}

/// Opt-in file capture for the actual container executing a Buck2 action.
/// Requires a fresh single-use container with no outer execution wrapper or
/// separate action mount namespace. For nested runtimes, run `NativeLink` and the
/// capture helper inside the execution container. Other build tools never start
/// capture. The helper uploads an archive before `NativeLink` cleans up the action.
///
/// The operator supplies the helper executable and its authenticated ingest
/// service. `NativeLink` sends one JSON record on stdin and expects `ready\n`
/// on stdout within 30 seconds. Closing stdin requests final capture; the helper
/// must exit only after durable acknowledgement, or exit unsuccessfully if the
/// archive is partial. See `nativelink-worker/README.md` for the control protocol.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct Buck2FileCaptureConfig {
    /// Absolute path to the compatible `buck2-file-capture` executable.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub executable: String,
    /// Authenticated ingest endpoint understood by the capture helper.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub gateway: String,
    /// Private token file. The helper excludes its path and hard-link aliases.
    /// The token is never included in the action environment or command line.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub token_file: String,
    /// Private directory for the helper's bounded on-disk metadata index.
    /// Mount a fresh empty volume here for each single-use container.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub state_directory: String,
    /// Identity of the actual executing container, normally the pod UID.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub container_id: String,
    /// Credential files/directories and platform configuration excluded from
    /// browsing. Their inode aliases and private secret mounts are also denied.
    #[serde(default, deserialize_with = "convert_vec_string_with_shellexpand")]
    pub protected_paths: Vec<String>,
    /// Internal CAS/cache paths excluded from browsing. Materialized action
    /// inputs hard-linked from these caches remain visible in the action tree.
    #[serde(default, deserialize_with = "convert_vec_string_with_shellexpand")]
    pub internal_paths: Vec<String>,
    /// Maximum final archive time before action cleanup proceeds. The helper
    /// records a partial capture on timeout; build success/failure is unchanged.
    /// Default: 120 seconds (zero selects this default).
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub finalize_timeout_s: usize,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent on/off switches of one worker; a nested struct per flag would not read better"
)]
#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct LocalWorkerConfig {
    /// Name of the worker. This is give a more friendly name to a worker for logging
    /// and metric publishing. This is also the prefix of the worker id
    /// (i.e. "{name}{uuidv6}").
    /// Default: {Index position in the workers list}
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub name: String,

    /// Endpoint which the worker will connect to the scheduler's `WorkerApiService`.
    pub worker_api_endpoint: EndpointConfig,

    /// The maximum time an action is allowed to run. If a task requests for a timeout
    /// longer than this time limit, the task will be rejected. Value in seconds.
    ///
    /// Default: 20 minutes
    #[serde(
        default,
        deserialize_with = "convert_duration_with_shellexpand",
        alias = "max_action_timeout"
    )]
    pub max_action_timeout_s: usize,

    /// Maximum time allowed for uploading action results to CAS after execution
    /// completes. If upload takes longer than this, the action fails with
    /// `DeadlineExceeded` and may be retried by the scheduler. Value in seconds.
    ///
    /// Default: 10 minutes
    #[serde(
        default,
        deserialize_with = "convert_duration_with_shellexpand",
        alias = "max_upload_timeout"
    )]
    pub max_upload_timeout_s: usize,

    /// Maximum time allowed for fetching an action's inputs into the worker's
    /// store and its directory before the command runs. A fetch that hangs
    /// (a store that never answers) otherwise holds the action's slot for
    /// good while the worker looks healthy; past this the action fails with
    /// `DeadlineExceeded` and may be retried by the scheduler, and a warning
    /// names the action every minute before that. Value in seconds.
    ///
    /// A fetch that fails with a transient store error is fetched again
    /// from this worker, with a pause doubling from 1 s to 30 s, for as
    /// long as this leaves room for another attempt, or the action's own
    /// timeout when that is shorter. The scheduler is not told of the
    /// retries: it ends an executing action on its own at the action's
    /// timeout plus `worker_timeout_s`, and at
    /// `max_action_executing_timeout_s`, so keep this below those or the
    /// action is requeued while the worker still retries.
    ///
    /// Default: 10 minutes
    #[serde(
        default,
        deserialize_with = "convert_duration_with_shellexpand",
        alias = "max_download_timeout"
    )]
    pub max_download_timeout_s: usize,

    /// How many input blobs this worker fetches into its store at once,
    /// across every action it is staging. Each fetch is a read from the
    /// CAS or the object store, so this bounds what one worker asks of
    /// them at a time: several cold actions share this many fetches
    /// instead of each opening its own. Blobs already in the store are
    /// linked without one. 0 shares no pool; each action fetches on its
    /// own.
    ///
    /// Without a pool, an action fetches up to 64 inputs at a time per
    /// directory of its input tree. Set this when several cold actions on
    /// one worker overload the CAS or the object store behind it; 64 is a
    /// good start. A worker without it logs a warning suggesting it, at
    /// most every 10 minutes, when an input fetch times out or fails with
    /// a transient error other than `DataLoss`.
    ///
    /// Default: 0 (no shared pool)
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_concurrent_input_downloads: usize,

    /// Maximum time to wait for action directory cleanup before timing out.
    /// Value in seconds.
    ///
    /// Default: 30 seconds
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub max_cleanup_wait_s: usize,

    /// Maximum backoff duration for exponential backoff when waiting for cleanup.
    /// Value in milliseconds.
    ///
    /// Default: 500 milliseconds
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub max_cleanup_backoff_ms: usize,

    /// Maximum number of inflight tasks this worker can cope with.
    ///
    /// Default: 0 (infinite tasks)
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_inflight_tasks: u64,

    /// Accept one action, upload its results, and exit the worker process.
    /// The worker advertises one execution slot and never accepts a second
    /// action, including after a scheduler disconnect. A launcher must create
    /// a fresh container and fresh writable volumes for its replacement.
    /// Use one local worker per process, with no scheduler or services other
    /// than health checks. Reusing the container or its writable volumes does
    /// not provide isolation from the previous action.
    ///
    /// Default: false
    #[serde(default)]
    pub single_use: bool,

    /// Capture full execution-container files for Buck2 builds only. Disabled
    /// when absent. Requires `single_use`; read the capture configuration's
    /// isolation requirements before enabling it for a worker pool.
    #[serde(default)]
    pub experimental_buck2_file_capture: Option<Buck2FileCaptureConfig>,

    /// If timeout is handled in `entrypoint` or another wrapper script.
    /// If set to true `NativeLink` will not honor the timeout the action requested
    /// and instead will always force kill the action after `max_action_timeout`
    /// has been reached. If this is set to false, the smaller value of the action's
    /// timeout and `max_action_timeout` will be used to which `NativeLink` will kill
    /// the action.
    ///
    /// The real timeout can be received via an environment variable set in:
    /// `EnvironmentSource::TimeoutMillis`.
    ///
    /// Example on where this is useful: `entrypoint` launches the action inside
    /// a docker container, but the docker container may need to be downloaded. Thus
    /// the timer should not start until the docker container has started executing
    /// the action. In this case, action will likely be wrapped in another program,
    /// like `timeout` and propagate timeouts via `EnvironmentSource::SideChannelFile`.
    ///
    /// Default: false (`NativeLink` fully handles timeouts)
    #[serde(default)]
    pub timeout_handled_externally: bool,

    /// On shutdown, tell the scheduler at once that this worker is draining
    /// so it stops dispatching here, then finish the running actions (up to
    /// `max_action_timeout_s`) and exit. Off, the worker finishes first and
    /// only then announces itself, while the scheduler keeps offering it
    /// work it has to refuse. Needs a scheduler that understands the drain
    /// flag (v1.7.3+); an older one removes the worker and requeues its
    /// actions as soon as the message arrives.
    /// Default: false
    #[serde(default)]
    pub drain_on_shutdown: bool,

    /// Kill an action that grows past its memory reservation before the
    /// pod's cgroup limit does. On cgroup v2 the container cgroup carries
    /// `memory.oom.group`, so the kernel's kill takes the worker and every
    /// action on it; this one takes only the offender and fails it with
    /// `FailedPrecondition`. The memory pressure kill reads a cgroup and is
    /// Linux only; on Windows the disk axis is not sampled, and a kill ends
    /// the action's own process rather than every process in it.
    /// Actions run through a persistent worker are not covered, since that
    /// process outlives the action and serves others. With either axis on,
    /// the worker reads an action's platform properties from the
    /// scheduler's `StartExecute` rather than the client's request, so a
    /// reservation the scheduler placed the action by is the one enforced
    /// and exported to the environment.
    #[serde(default)]
    pub resource_enforcement: Option<ResourceEnforcementConfig>,

    /// The command to execute on every execution request. This will be parsed as
    /// a command + arguments (not shell).
    /// Example: "run.sh" and a job with command: "sleep 5" will result in a
    /// command like: "run.sh sleep 5".
    /// Default: {Use the command from the job request}.
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub entrypoint: String,

    /// An optional script to run before every action is processed on the worker.
    /// The value should be the full path to the script to execute and will pause
    /// all actions on the worker if it returns an exit code other than 0.
    /// If not set, then the worker will never pause and will continue to accept
    /// jobs according to the scheduler configuration.
    /// This is useful, for example, if the worker should not take any more
    /// actions until there is enough resource available on the machine to
    /// handle them.
    pub experimental_precondition_script: Option<String>,

    /// Milliseconds the precondition script may run before it counts as
    /// failed and the action is refused as backpressure. A script that
    /// hangs used to hold the action forever.
    /// Default: 30000
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub precondition_timeout_ms: u64,

    /// Bytes of an action's stdout or stderr kept in memory. Past this the
    /// output spills to a file under the action directory and is uploaded
    /// from disk, so an action that prints gigabytes does not grow the
    /// worker by that much. 0 keeps everything in memory.
    /// Default: 0
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_captured_output_bytes: u64,

    /// Seconds between sweeps of `work_directory` for action directories no
    /// running action owns, left by a cleanup that failed. 0 disables the
    /// sweep; the directory is still purged at startup.
    /// Default: 0
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub orphan_sweep_interval_s: u64,

    /// Milliseconds between the SIGTERM a timed-out or cancelled action
    /// receives and the SIGKILL that follows if it is still running, so a
    /// tool that handles SIGTERM can write its own cleanup. The memory
    /// reservation kill skips the grace and sends SIGKILL at once. Linux
    /// only; elsewhere the kill is immediate. 0 takes the default.
    /// Default: 5000
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub kill_grace_ms: u64,

    /// Give every action its own `TMPDIR` under its action directory,
    /// removed with it, so concurrent actions writing the same file name
    /// under `$TMPDIR` no longer collide. A `TMPDIR` in the action's own
    /// environment still wins. Code that hard-codes `/tmp` is unaffected.
    /// Default: true
    #[serde(default = "default_true")]
    pub set_tmpdir: bool,

    /// Bazel persistent workers: how an action whose platform properties
    /// carry `supports-workers=1` is run and how the pool of worker
    /// processes is sized. Unset takes every default below.
    #[serde(default)]
    pub persistent_workers: Option<PersistentWorkersConfig>,

    /// Underlying CAS store that the worker will use to download CAS artifacts.
    /// This store must be a `FastSlowStore`. The `fast` store must be a
    /// `FileSystemStore` because it will use hardlinks when building out the files
    /// instead of copying the files. The slow store must eventually resolve to the
    /// same store the scheduler/client uses to send job requests.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub cas_fast_slow_store: StoreRefName,

    /// Configuration for uploading action results.
    #[serde(default)]
    pub upload_action_result: UploadActionResultConfig,

    /// The directory work jobs will be executed from. This directory will be fully
    /// managed by the worker service and will be purged on startup.
    /// This directory and the directory referenced in `local_filesystem_store_ref`'s
    /// `stores::FilesystemStore::content_path` must be on the same filesystem.
    /// Hardlinks will be used when placing files that are accessible to the jobs
    /// that are sourced from `local_filesystem_store_ref`'s `content_path`.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub work_directory: String,

    /// Properties of this worker. This configuration will be sent to the scheduler
    /// and used to tell the scheduler to restrict what should be executed on this
    /// worker.
    pub platform_properties: HashMap<String, WorkerProperty>,

    /// Derive the CPU and memory properties from the worker's own cgroup
    /// limits instead of the values above; see `CapacityConfig`. When the
    /// cgroup cannot be read (not Linux, cgroup v1, no permission) the
    /// values above stand and a warning says so; if they carry neither
    /// property the worker refuses to start rather than register unable to
    /// take any action that asks for CPU or memory.
    #[serde(default)]
    pub capacity: Option<CapacityConfig>,

    /// An optional mapping of environment names to set for the execution
    /// as well as those specified in the action itself. If set, will set each
    /// key as an environment variable before executing the job with the value
    /// of the environment variable being the value of the property of the
    /// action being executed of that name or the fixed value.
    pub additional_environment: Option<HashMap<String, EnvironmentSource>>,

    /// Optional directory cache configuration for improving performance by caching
    /// reconstructed input directories and using hardlinks instead of rebuilding
    /// them from CAS for every action.
    /// Default: None (directory cache disabled)
    pub directory_cache: Option<DirectoryCacheConfig>,

    /// Optional and experimental: lease every digest in an active action's
    /// input Merkle closure in the worker's locally eviction-managed CAS
    /// tiers (the `cas_fast_slow_store`'s filesystem-backed stores) until the
    /// action has finished cleanup. This prevents `Lost inputs no longer
    /// available remotely` failures caused by local fast-tier eviction while
    /// inputs are being materialized under cache pressure.
    ///
    /// Trade-off: while leases are held, a local filesystem tier may
    /// temporarily exceed its configured `max_bytes` / `max_count` eviction
    /// limits; normal eviction resumes once the action's leases are released.
    /// Operators should leave headroom on the underlying disk when enabling
    /// this.
    ///
    /// Default: false (eviction behavior is unchanged)
    #[serde(default)]
    pub experimental_active_input_leases: bool,

    /// Reuse immutable input directories through read-only bind mounts on Linux.
    /// The worker prepares each selected subtree once per REAPI `Directory`
    /// digest in the directory cache, then mounts it into each action instead of
    /// recreating its filesystem entries.
    ///
    /// Paths are relative to the action input root, for example `["out/x/sdk"]`.
    /// They must be nonempty, canonical, and non-overlapping. Absolute paths,
    /// empty path components, and `.`/`..` components are rejected.
    /// Requires `directory_cache`, `use_namespaces: true`, and
    /// `use_mount_namespace: true`.
    ///
    /// Actions declaring overlapping outputs, working inside a selected subtree,
    /// or using a persistent worker fall back to ordinary materialization.
    /// Output paths reached through symlink aliases cannot be checked for
    /// overlap. Use this only with trusted workloads that do not modify the
    /// selected trees; read-only input mounts are not a security boundary.
    ///
    /// Select trees whose contents (and Directory digest) are reused across
    /// many actions. Trees that change per action add cache construction work
    /// without reuse. When mounts apply, other inputs use ordinary downloads,
    /// bypassing both root and subtree directory caching for that action.
    /// Failed mount preparation retries once without mounts; a failed mount
    /// syscall still aborts the action before its command runs.
    ///
    /// An action can remount its inputs writable in its own user namespace
    /// and corrupt the shared cache. Later actions can then produce incorrect
    /// results from those inputs. Enable only for trusted commands.
    ///
    /// Cache entries remain pinned until action cleanup and may temporarily
    /// exceed the cache's eviction budget. Pinned bytes have no separate limit,
    /// so concurrent distinct trees can exhaust the disk. Leave headroom.
    ///
    /// Default: [] (disabled)
    #[serde(default)]
    pub experimental_readonly_input_mounts: Vec<String>,

    /// Whether to use namespaces to isolate the execution. This is only available
    /// on Linux. It is highly recommended as it avoids a number of issues with
    /// zombie processes and also provides additional hermeticity. If explicitly set
    /// to true and it is not supported the worker will exit with an error.
    ///
    /// The action runs as PID 1 of its own PID namespace, and `/proc` is
    /// remounted inside it, so the processes listed there are the action's
    /// own, each under the PID the action knows it by: a process reading
    /// `/proc/<pid>` for a child it forked finds that child, not whichever
    /// host process has the number. If the worker's environment refuses that
    /// mount (an unprivileged container that masks paths under `/proc` does)
    /// the worker logs a warning at startup and actions see the host's
    /// `/proc` instead.
    ///
    /// Note: this will fail for non-privileged Dockerised workers, as workers in
    /// Docker don't have permissions to make a new user namespace. Privileged
    /// containers can do this.
    ///
    /// Default: False.
    pub use_namespaces: Option<bool>,

    /// Whether to use a mount namespace to isolate the worker root. This is only
    /// available on Linux and when `use_namespaces` is true. It is highly recommended
    /// provides additional hermeticity. If explicitly set to true and it is not
    /// supported or `use_namespaces` is not set to true the worker will exit with an
    /// error.
    /// Default: False.
    pub use_mount_namespace: Option<bool>,

    /// Whether to give each action a private `/tmp` inside its mount
    /// namespace: the action's own `tmp` directory (the one `TMPDIR` points
    /// at) is bound over `/tmp`, so tools that hardcode `/tmp` write there
    /// too. Concurrent actions no longer collide on predictable paths under
    /// `/tmp`, nothing leaks between actions through it, and what an action
    /// writes there is removed with the action.
    ///
    /// This is only available on Linux and requires `use_namespaces` and
    /// `use_mount_namespace` to be true. If explicitly set to true without
    /// both the worker will exit with an error. Set it to false to keep the
    /// host's `/tmp` visible to actions, for example when a tool they need
    /// lives there. The bound `/tmp` carries the work volume's mount options
    /// (`noexec`, `nodev`, `nosuid` if the volume has them), not the host
    /// `/tmp`'s.
    ///
    /// The private `/tmp` lives on the worker's disk under the work
    /// directory, not in memory, so it is bounded by the volume that holds
    /// the work directory and seen by the disk guard, and a tool filling it
    /// cannot take the worker's memory with it.
    ///
    /// If `/tmp` does not exist on the worker, for example in a minimal
    /// container image, there is nothing for actions to collide on, so the
    /// default is False there and a warning is logged at startup.
    ///
    /// Default: True when `use_mount_namespace` is true and `/tmp` exists,
    /// otherwise False.
    pub isolate_tmp: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct DirectoryCacheConfig {
    /// Maximum number of cached directories.
    /// Default: 1000
    #[serde(default = "default_directory_cache_max_entries")]
    pub max_entries: usize,

    /// Maximum total size in bytes for all cached directories (0 = unlimited).
    /// Default: 10737418240 (10 GB)
    #[serde(
        default = "default_directory_cache_max_size_bytes",
        deserialize_with = "convert_data_size_with_shellexpand"
    )]
    pub max_size_bytes: u64,

    /// Base directory for cache storage. This directory will be managed by
    /// the worker and should be on the same filesystem as `work_directory`.
    /// Default: `{work_directory}/../directory_cache`
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub cache_root: String,

    /// Optional and experimental: additionally cache every subdirectory by
    /// its own `Directory` digest, not just the root directory. REAPI Merkle
    /// nodes are content-addressed, so a subtree that is byte-identical
    /// between two different roots has the same digest and can be
    /// materialized with a single hardlink pass instead of being rebuilt
    /// from the CAS. This makes the common "one input file changed out of
    /// thousands" case reuse every unchanged subtree.
    ///
    /// Note: subtree caching multiplies the entry COUNT in the cache (every
    /// distinct subdirectory becomes its own entry, subject to the normal
    /// eviction budgets), so operators enabling this should raise
    /// `max_entries` accordingly (for example 10x). To avoid multiplying the
    /// SIZE accounting too, an entry's recorded size covers only bytes not
    /// owned by a descendant entry, so the cache's size total approximates
    /// unique materialized bytes rather than counting each file once per
    /// ancestor level.
    ///
    /// Trade-off: cold constructions pay roughly one extra hardlink pass per
    /// tree level (measured ~10% cold overhead at depth 3, within run
    /// variance) in exchange for the churn-path reuse above.
    ///
    /// Default: false (only root directories are cached; existing behavior)
    #[serde(default)]
    pub experimental_subtree_caching: bool,

    /// Maximum number of concurrent slow-store fetches across ALL directory
    /// constructions of this cache. This bound protects backing stores from
    /// RPC storms: per-level construction concurrency compounds
    /// multiplicatively across tree levels and concurrent actions.
    ///
    /// Interaction with read coalescing: this semaphore fragments coalesced
    /// batches to at most this many items and serializes fetch waves for
    /// trees with many tiny files. Deployments using
    /// `experimental_read_batching` on the underlying grpc store can raise
    /// this substantially (for example 512), since batching collapses the
    /// RPC count. Must be greater than 0.
    ///
    /// Default: 64
    #[serde(default = "default_directory_cache_max_concurrent_fetches")]
    pub max_concurrent_fetches: usize,
    /// On a directory-cache miss, prefetch every `Directory` proto of the
    /// tree with one logical `GetTree` traversal instead of fetching protos
    /// level by level (which costs one round trip per tree DEPTH). The
    /// [REAPI request contract] permits servers to impose their own limit even
    /// when no page size is specified, and the [REAPI response contract]
    /// requires clients to continue with the returned page token. A paginated
    /// traversal may therefore use multiple RPCs. Only takes effect when the
    /// slow tier is a `grpc` store;
    /// any prefetch failure falls back to the per-level path. Worthwhile when
    /// worker-to-CAS latency is non-trivial and trees are deep; measured 5-34x
    /// on the proto phase at 5-25ms RTT. Note the serving CAS pays the tree walk
    /// against its own backend, so its directory protos should be served
    /// from a fast tier.
    ///
    /// [REAPI request contract]: https://github.com/bazelbuild/remote-apis/blob/becdd8f9ff811df88a22d3eadd6341753d51d167/build/bazel/remote/execution/v2/remote_execution.proto#L1932-L1942
    /// [REAPI response contract]: https://github.com/bazelbuild/remote-apis/blob/becdd8f9ff811df88a22d3eadd6341753d51d167/build/bazel/remote/execution/v2/remote_execution.proto#L1961-L1965
    /// Default: false
    #[serde(default)]
    pub experimental_get_tree_prefetch: bool,

    /// Optional and experimental, macOS only: build the cache's copy of an
    /// executable input file as an APFS `clonefile(2)` clone of the worker's
    /// local CAS blob instead of reading the blob into memory and writing a
    /// second copy of it.
    ///
    /// A non-executable input is hardlinked from the `filesystem` store. An
    /// executable one cannot be: the hardlink would share the blob's inode,
    /// and setting its executable bit would change the mode of the blob
    /// itself. It is therefore given a private inode, which without this
    /// option means buffering the whole blob in memory and storing its bytes
    /// a second time. Bazel marks every file it outputs as executable, so on
    /// a Bazel workload that can be every input. A clone is also a private
    /// inode, with its own mode, but it shares the blob's data blocks
    /// copy-on-write: nothing is read or written.
    ///
    /// Requirements: the fast store of the worker's CAS is a `filesystem`
    /// store, and its `content_path` and this cache's `cache_root` are on
    /// the same APFS volume. This is checked once at startup; if a clone is
    /// not possible a warning is logged and executable files are copied as
    /// before. A single file that cannot be cloned is copied as well. On
    /// other operating systems the option logs a warning and does nothing.
    ///
    /// What changes for an action: a cloned file keeps the modification time
    /// of the CAS blob (as a hardlinked non-executable input does) where a
    /// copy has the time the cache entry was built. A clone would also carry
    /// the blob's extended attributes and BSD file flags, so a blob that has
    /// either is copied instead.
    ///
    /// What does not change: `max_size_bytes` still counts the full size of
    /// every file in an entry, so the cache evicts exactly as before even
    /// though clones take no space of their own. Conversely, evicting a blob
    /// from the `filesystem` store frees no disk space while a cache entry
    /// or an action directory still holds a clone of it. Disk use is never
    /// higher than with copies. Each clone is still a file, so the number of
    /// files in the cache is unchanged.
    ///
    /// Default: false (executable files are copied; existing behavior)
    #[serde(default)]
    pub experimental_clone_executables: bool,
}

const fn default_directory_cache_max_entries() -> usize {
    1000
}

const fn default_directory_cache_max_concurrent_fetches() -> usize {
    64
}

const fn default_directory_cache_max_size_bytes() -> u64 {
    10 * 1024 * 1024 * 1024 // 10 GB
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum WorkerConfig {
    /// A worker type that executes jobs locally on this machine.
    Local(LocalWorkerConfig),
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct GlobalConfig {
    /// Maximum number of open files that can be opened at one time.
    /// This value is not strictly enforced, it is a best effort. Some internal libraries
    /// open files or read metadata from a files which do not obey this limit, however
    /// the vast majority of cases will have this limit be honored.
    /// This value must be larger than `ulimit -n` to have any effect.
    /// Any network open file descriptors is not counted in this limit, but is counted
    /// in the kernel limit. It is a good idea to set a very large `ulimit -n`.
    /// Note: This value must be greater than 10.
    ///
    /// Default: 24576 (= 24 * 1024)
    #[serde(deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_open_files: usize,

    /// Default hash function to use while uploading blobs to the CAS when not set
    /// by client.
    ///
    /// Default: `ConfigDigestHashFunction::sha256`
    pub default_digest_hash_function: Option<ConfigDigestHashFunction>,

    /// Default digest size to use for health check when running
    /// diagnostics checks. Health checks are expected to use this
    /// size for filling a buffer that is used for creation of
    /// digest.
    ///
    /// Default: 1024*1024 (1MiB)
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub default_digest_size_health_check: usize,
}

pub type StoreConfig = NamedConfig<StoreSpec>;
pub type SchedulerConfig = NamedConfig<SchedulerSpec>;

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct CasConfig {
    /// List of stores available to use in this config.
    /// The keys can be used in other configs when needing to reference a store.
    pub stores: Vec<StoreConfig>,

    /// Worker configurations used to execute jobs.
    pub workers: Option<Vec<WorkerConfig>>,

    /// List of schedulers available to use in this config.
    /// The keys can be used in other configs when needing to reference a
    /// scheduler.
    pub schedulers: Option<Vec<SchedulerConfig>>,

    /// Servers to setup for this process.
    pub servers: Vec<ServerConfig>,

    /// Experimental - Origin events configuration. This is the service that will
    /// collect and publish nativelink events to a store for processing by an
    /// external service.
    pub experimental_origin_events: Option<OriginEventsSpec>,

    /// Any global configurations that apply to all modules live here.
    pub global: Option<GlobalConfig>,
}

impl CasConfig {
    const ZSTD_COMPRESSION_DOCS_URL: &'static str =
        "https://docs.nativelink.com/configuration/compression";

    /// # Errors
    ///
    /// Will return `Err` if we can't load the file.
    pub fn try_from_json5_file(config_file: &str) -> Result<Self, Error> {
        let json_contents = std::fs::read_to_string(config_file)
            .err_tip(|| format!("Could not open config file {config_file}"))?;
        Self::try_from_json5_str(&json_contents)
    }

    pub fn try_from_json5_str(json_contents: &str) -> Result<Self, Error> {
        let mut config: Self = serde_json5::from_str(json_contents)?;
        for server in &config.servers {
            if let Some(services) = &server.services {
                Self::check_store_conflict(services)?;
            }
        }
        config.validate_single_use_worker()?;
        config.apply_zstd_grpc_store_defaults();
        Ok(config)
    }

    /// Map each instance name served by an Action Cache service to whether
    /// that instance is read only. Collected across all server blocks, the
    /// same way the capabilities service advertises instances that may be
    /// served from a different block. A later entry for an instance wins.
    #[must_use]
    pub fn action_cache_read_only_by_instance(&self) -> HashMap<InstanceName, bool> {
        self.servers
            .iter()
            .filter_map(|server| server.services.as_ref())
            .filter_map(|services| services.ac.as_deref())
            .flatten()
            .map(|ac| (ac.instance_name.clone(), ac.read_only))
            .collect()
    }

    /// True when this process runs shared (non-`single_use`) workers *and* at
    /// least one Action Cache instance accepts client writes (`read_only` is
    /// false). That combination is unsafe for untrusted builds: a job can
    /// leave state on a worker that the next job inherits, and a client can
    /// write an `ActionResult` that later builds trust. It is only a warning,
    /// because it is the right configuration when every build is trusted.
    #[must_use]
    pub fn shares_workers_with_writable_action_cache(&self) -> bool {
        let has_shared_worker = self
            .workers
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|WorkerConfig::Local(worker)| !worker.single_use);
        let action_cache_accepts_client_writes = self
            .servers
            .iter()
            .filter_map(|server| server.services.as_ref())
            .filter_map(|services| services.ac.as_deref())
            .flatten()
            .any(|ac| !ac.read_only);
        has_shared_worker && action_cache_accepts_client_writes
    }

    fn validate_single_use_worker(&self) -> Result<(), Error> {
        let workers = self.workers.as_deref().unwrap_or_default();
        for WorkerConfig::Local(worker) in workers {
            if let Some(capture) = &worker.experimental_buck2_file_capture {
                if !worker.single_use
                    || !worker.entrypoint.is_empty()
                    || worker.use_mount_namespace == Some(true)
                {
                    return Err(make_input_err!(
                        "Buck2 file capture requires a single_use worker inside the execution container, without an entrypoint wrapper or separate action mount namespace"
                    ));
                }
                for path in [
                    &capture.executable,
                    &capture.token_file,
                    &capture.state_directory,
                ]
                .into_iter()
                .chain(capture.protected_paths.iter())
                .chain(capture.internal_paths.iter())
                {
                    // These paths belong to the Linux execution container,
                    // even when an operator validates the config on Windows.
                    if !path.starts_with('/')
                        || path.split('/').all(|part| part.is_empty() || part == ".")
                        || path.contains('\0')
                        || path.split('/').any(|part| part == "..")
                    {
                        return Err(make_input_err!(
                            "Buck2 capture paths must be absolute and cannot select the filesystem root"
                        ));
                    }
                }
                if capture.gateway.is_empty()
                    || capture.container_id.is_empty()
                    || capture.finalize_timeout_s > 3600
                {
                    return Err(make_input_err!(
                        "Buck2 capture requires a gateway, container identity, and finalization timeout no greater than one hour"
                    ));
                }
            }
        }
        if !workers.iter().any(|worker| match worker {
            WorkerConfig::Local(worker) => worker.single_use,
        }) {
            return Ok(());
        }
        if workers.len() != 1 || self.schedulers.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(make_input_err!(
                "single_use requires exactly one worker and no scheduler in the process"
            ));
        }
        for services in self
            .servers
            .iter()
            .filter_map(|server| server.services.as_ref())
        {
            let ServicesConfig {
                cas,
                ac,
                capabilities,
                execution,
                bytestream,
                fetch,
                push,
                worker_api,
                experimental_bep,
                admin,
                health: _,
            } = services;
            if cas.is_some()
                || ac.is_some()
                || capabilities.is_some()
                || execution.is_some()
                || bytestream.is_some()
                || fetch.is_some()
                || push.is_some()
                || worker_api.is_some()
                || experimental_bep.is_some()
                || admin.is_some()
            {
                return Err(make_input_err!(
                    "single_use worker processes may expose only health services"
                ));
            }
        }
        Ok(())
    }

    fn zstd_wire_compression_enabled_anywhere(&self) -> bool {
        if self
            .servers
            .iter()
            .filter_map(|server| server.services.as_ref())
            .filter_map(|services| services.capabilities.as_deref())
            .flatten()
            .any(|capabilities| capabilities.remote_cache_compression)
        {
            return true;
        }

        self.stores.iter().any(|store| {
            let mut enabled = false;
            store.spec.visit_grpc_specs(&mut |grpc| {
                enabled |= matches!(grpc.store_type, StoreType::Cas)
                    && grpc.experimental_remote_cache_compression == Some(true);
            });
            enabled
        })
    }

    fn apply_zstd_grpc_store_defaults(&mut self) {
        let zstd_enabled_anywhere = self.zstd_wire_compression_enabled_anywhere();

        for store in &mut self.stores {
            let store_name = store.name.as_str();
            store.spec.visit_grpc_specs_mut(&mut |grpc| {
                if !matches!(grpc.store_type, StoreType::Cas) {
                    if grpc.experimental_remote_cache_compression == Some(true) {
                        warn!(
                            store = store_name,
                            instance_name = grpc.instance_name,
                            "'experimental_remote_cache_compression' is enabled on a non-CAS \
                             gRPC store, where it has no effect: REAPI zstd wire compression \
                             only applies to CAS blob transfers. Enable it on the CAS gRPC \
                             stores or a capabilities instance instead. See {}",
                            Self::ZSTD_COMPRESSION_DOCS_URL,
                        );
                    }
                    return;
                }

                if !zstd_enabled_anywhere {
                    return;
                }

                match grpc.experimental_remote_cache_compression {
                    None => grpc.experimental_remote_cache_compression = Some(true),
                    Some(false) => warn!(
                        store = store_name,
                        instance_name = grpc.instance_name,
                        "Zstd wire compression is enabled elsewhere, but this eligible CAS gRPC \
                         store explicitly disables it. Unless this upstream cannot use zstd, \
                         enable 'experimental_remote_cache_compression' for substantially faster \
                         transfers of compressible artifacts. See {}",
                        Self::ZSTD_COMPRESSION_DOCS_URL,
                    ),
                    Some(true) => {}
                }
            });
        }
    }

    fn check_store_conflict(services: &ServicesConfig) -> Result<(), Error> {
        if let Some(cas_config) = &services.cas
            && let Some(ac_config) = &services.ac
        {
            // Create a hashmap from the CAS configuration for quick lookup
            let cas_store_map: HashMap<_, _> = cas_config
                .iter()
                .map(|with_instance_name| {
                    (
                        &with_instance_name.instance_name,
                        &with_instance_name.cas_store,
                    )
                })
                .collect();

            for with_instance_name in ac_config {
                if let Some(cas_store) = cas_store_map.get(&with_instance_name.instance_name)
                    && cas_store == &&with_instance_name.ac_store
                {
                    return Err(make_err!(
                        Code::InvalidArgument,
                        "CAS and AC use the same store '{}' in the config",
                        cas_store
                    ));
                }
            }
        }
        Ok(())
    }

    /// Structural validation of every store/scheduler name reference in this
    /// config. This is a pure, read-only check: it constructs no stores, binds
    /// no sockets, opens no connections, and creates no directories. It backs
    /// `nativelink --check` and runs again at the start of normal startup, so
    /// the two paths reject exactly the same configs.
    ///
    /// Every store name referenced by a worker, by a server service, or by a
    /// scheduler, and every scheduler name referenced by a service, must
    /// resolve to a store declared in `stores` / a scheduler declared in
    /// `schedulers`. Store references made *inside* a declared store's spec
    /// (the `ref_store` wrappers such as `fast_slow`, `dedup`,
    /// `completeness_checking`, ...) are resolved for *every* declared store,
    /// whether or not a consumer is wired to it, because startup constructs
    /// every declared store and `RefStore::post_init` rejects a dangling name
    /// regardless of use.
    ///
    /// An empty `stores` list or an absent `schedulers` list is not special:
    /// a reference into an empty registry is unresolved, exactly as it is at
    /// startup. Illustrative fragments that rely on definitions living
    /// elsewhere are a test-fixture concern, not a deployment one.
    ///
    /// All problems are collected and reported together, each with a path
    /// such as `servers[0].services.cas[main].cas_store references
    /// undefined store 'CAS_STORE'`, rather than failing on the first one.
    ///
    /// Duplicate store names are also reported, because the store manager
    /// refuses to register a second store under a name it already holds, so
    /// startup rejects them too.
    ///
    /// Duplicate scheduler names are not an error here: startup accepts them
    /// and the last definition wins. [`Self::problems`] reports them as
    /// warnings.
    ///
    /// # Errors
    ///
    /// Returns `Err(Code::InvalidArgument)` listing every error-severity
    /// problem from [`Self::problems`], if there are any.
    pub fn validate_references(&self) -> Result<(), Error> {
        let errors: Vec<ConfigProblem> = self
            .problems()
            .into_iter()
            .filter(|problem| problem.severity == Severity::Error)
            .collect();
        match errors.len() {
            0 => Ok(()),
            count => Err(make_err!(
                Code::InvalidArgument,
                "{count} configuration problem{}:\n  - {}",
                if count == 1 { "" } else { "s" },
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n  - ")
            )),
        }
    }

    /// Every problem [`Self::validate_references`] checks for, in the order
    /// the config is walked, followed by the warnings: things startup
    /// accepts but that are probably a mistake.
    ///
    /// An error-severity problem means startup refuses the config. A
    /// warning means it starts, so `--check` reports it and still passes.
    #[must_use]
    pub fn problems(&self) -> Vec<ConfigProblem> {
        let mut problems: Vec<ConfigProblem> = Vec::new();

        // Declared registries. Collision on insert is a duplicate name.
        let mut store_names: HashSet<&str> = HashSet::new();
        for store in &self.stores {
            if !store_names.insert(store.name.as_str()) {
                problems.push(ConfigProblem::error(
                    "",
                    format!("duplicate store name '{}'", store.name),
                ));
            }
        }
        let scheduler_names: HashSet<&str> = self
            .schedulers
            .iter()
            .flatten()
            .map(|scheduler| scheduler.name.as_str())
            .collect();

        let mut checker = ReferenceChecker {
            store_names,
            scheduler_names,
            problems,
        };

        // Startup names an unnamed worker `worker_{index}` and refuses a name
        // used twice.
        let mut worker_names: HashMap<String, usize> = HashMap::new();
        for (worker_idx, worker) in self.workers.iter().flatten().enumerate() {
            let WorkerConfig::Local(local) = worker;
            let worker_name = if local.name.is_empty() {
                format!("worker_{worker_idx}")
            } else {
                local.name.clone()
            };
            if let Some(first_idx) = worker_names.get(&worker_name) {
                checker.problems.push(ConfigProblem::error(
                    format!("workers[{worker_idx}]"),
                    format!("has the same name as workers[{first_idx}]: '{worker_name}'"),
                ));
            } else {
                worker_names.insert(worker_name, worker_idx);
            }
            checker.store(
                &format!("workers[{worker_idx}].local.cas_fast_slow_store"),
                &local.cas_fast_slow_store,
            );
            if let Some(ac_store) = &local.upload_action_result.ac_store {
                checker.store(
                    &format!("workers[{worker_idx}].local.upload_action_result.ac_store"),
                    ac_store,
                );
            }
            if let Some(historical) = &local.upload_action_result.historical_results_store {
                checker.store(
                    &format!(
                        "workers[{worker_idx}].local.upload_action_result.historical_results_store"
                    ),
                    historical,
                );
            }
        }

        if let Some(origin_events) = &self.experimental_origin_events {
            checker.store(
                "experimental_origin_events.publisher.store",
                &origin_events.publisher.store,
            );
        }

        // Startup binds every listener; an address that does not parse, or one
        // two servers share, fails there. Port 0 asks the OS for a free port,
        // so it can repeat.
        let mut addresses: HashMap<SocketAddr, usize> = HashMap::new();
        for (server_idx, server) in self.servers.iter().enumerate() {
            let ListenerConfig::Http(http) = &server.listener;
            let address = &http.socket_address;
            match address.parse::<SocketAddr>() {
                Err(_) => checker.problems.push(ConfigProblem::error(
                    format!("servers[{server_idx}].listener.http.socket_address"),
                    format!("is not a valid address: '{address}'"),
                )),
                Ok(parsed) if parsed.port() != 0 => {
                    if let Some(first_idx) = addresses.get(&parsed) {
                        checker.problems.push(ConfigProblem::error(
                            format!("servers[{server_idx}]"),
                            format!(
                                "listens on the same address as servers[{first_idx}]: '{address}'"
                            ),
                        ));
                    } else {
                        addresses.insert(parsed, server_idx);
                    }
                }
                Ok(_) => {}
            }
        }

        for (server_idx, server) in self.servers.iter().enumerate() {
            let prefix = format!("servers[{server_idx}].services");
            let Some(services) = &server.services else {
                // Startup refuses a server with nothing to serve.
                checker.problems.push(ConfigProblem::error(
                    format!("servers[{server_idx}]"),
                    "has no 'services'",
                ));
                continue;
            };
            for entry in services.cas.iter().flatten() {
                let name = instance_label(&entry.instance_name);
                checker.store(&format!("{prefix}.cas[{name}].cas_store"), &entry.cas_store);
                if let Some(index_store) = entry
                    .experimental_chunking
                    .as_ref()
                    .and_then(|chunking| chunking.index_store.as_ref())
                {
                    checker.store(
                        &format!("{prefix}.cas[{name}].experimental_chunking.index_store"),
                        index_store,
                    );
                }
            }
            for entry in services.ac.iter().flatten() {
                let name = instance_label(&entry.instance_name);
                checker.store(&format!("{prefix}.ac[{name}].ac_store"), &entry.ac_store);
            }
            for entry in services.execution.iter().flatten() {
                let name = instance_label(&entry.instance_name);
                checker.store(
                    &format!("{prefix}.execution[{name}].cas_store"),
                    &entry.cas_store,
                );
                checker.scheduler(
                    &format!("{prefix}.execution[{name}].scheduler"),
                    &entry.scheduler,
                );
            }
            for entry in services.capabilities.iter().flatten() {
                if let Some(remote_execution) = &entry.remote_execution {
                    let name = instance_label(&entry.instance_name);
                    checker.scheduler(
                        &format!("{prefix}.capabilities[{name}].remote_execution.scheduler"),
                        &remote_execution.scheduler,
                    );
                }
            }
            for entry in services.bytestream.iter().flatten() {
                let name = instance_label(&entry.instance_name);
                checker.store(
                    &format!("{prefix}.bytestream[{name}].cas_store"),
                    &entry.cas_store,
                );
            }
            for entry in services.fetch.iter().flatten() {
                let name = instance_label(&entry.instance_name);
                checker.store(
                    &format!("{prefix}.fetch[{name}].fetch_store"),
                    &entry.fetch_store,
                );
                if let Some(downloader) = &entry.downloader {
                    checker.store(
                        &format!("{prefix}.fetch[{name}].downloader.cas_store"),
                        &downloader.cas_store,
                    );
                }
            }
            for entry in services.push.iter().flatten() {
                let name = instance_label(&entry.instance_name);
                checker.store(
                    &format!("{prefix}.push[{name}].push_store"),
                    &entry.push_store,
                );
            }
            if let Some(worker_api) = &services.worker_api {
                let path = format!("{prefix}.worker_api.scheduler");
                checker.scheduler(&path, &worker_api.scheduler);
                // Startup records a worker scheduler only for declarations
                // that have one, so if a name is declared twice, any
                // declaration that accepts workers is enough.
                let mut declared = self
                    .schedulers
                    .iter()
                    .flatten()
                    .filter(|scheduler| scheduler.name == worker_api.scheduler)
                    .peekable();
                if declared.peek().is_some()
                    && !declared.any(|scheduler| accepts_workers(&scheduler.spec))
                {
                    checker.problems.push(ConfigProblem::error(
                        path,
                        format!(
                            "references scheduler '{}', which forwards to a remote \
                             scheduler and cannot accept workers",
                            worker_api.scheduler
                        ),
                    ));
                }
            }
            if let Some(bep) = &services.experimental_bep {
                checker.store(&format!("{prefix}.experimental_bep.store"), &bep.store);
            }
        }

        // Scheduler specs themselves reference stores (and nest further
        // schedulers); walk every declared scheduler so those references are
        // resolved too.
        for scheduler in self.schedulers.iter().flatten() {
            checker.scheduler_spec(&format!("schedulers[{}]", scheduler.name), &scheduler.spec);
        }

        // Resolve the `ref_store` names nested inside every declared store.
        // Startup constructs all of them and runs `post_init` on each, so an
        // unused store with a dangling `ref_store` still fails to boot.
        for store in &self.stores {
            checker.store_internal_refs(&store.name, &store.spec);
        }

        // Startup builds every declared scheduler and keeps them in maps
        // keyed by name, so a later definition replaces an earlier one for
        // each role it fills (action scheduler, worker scheduler). The
        // earlier one is then partly or wholly unused. A warning, not an
        // error, because such a config starts. Reported once per name.
        let mut seen: HashSet<&str> = HashSet::new();
        let mut duplicates: Vec<&str> = Vec::new();
        for scheduler in self.schedulers.iter().flatten() {
            let name = scheduler.name.as_str();
            if !seen.insert(name) && !duplicates.contains(&name) {
                duplicates.push(name);
            }
        }
        for name in duplicates {
            checker.problems.push(ConfigProblem::warning(
                "",
                format!(
                    "scheduler name '{name}' is declared more than once; later \
                     definitions replace earlier ones"
                ),
            ));
        }

        checker.problems
    }
}

/// How serious a [`ConfigProblem`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Startup refuses the config.
    Error,
    /// Startup accepts the config, but it is probably not what was meant.
    Warning,
}

/// One problem [`CasConfig::problems`] found in a config.
///
/// Its `Display` form is the line `nativelink --check` and startup print:
/// the path, a space and the message, or the message alone when the
/// problem belongs to no single field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProblem {
    pub severity: Severity,
    /// Where in the config the problem is, such as
    /// `servers[0].services.cas[main].cas_store`. Empty when the problem
    /// belongs to no single field, such as a name declared twice.
    pub path: String,
    /// What is wrong, such as `references undefined store 'CAS'`.
    pub message: String,
}

impl ConfigProblem {
    fn error(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            path: path.into(),
            message: message.into(),
        }
    }

    fn warning(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl core::fmt::Display for ConfigProblem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{} {}", self.path, self.message)
        }
    }
}

/// How an instance name appears in a problem path: as written, or `""` when it
/// is empty, so `cas[""]` does not read as a missing index.
const fn instance_label(name: &str) -> &str {
    if name.is_empty() { "\"\"" } else { name }
}

/// Whether startup builds a worker scheduler for `spec`, which the worker API
/// service needs. Only a `simple` scheduler accepts workers; the wrapping
/// schedulers pass through whatever their nested scheduler provides, and a
/// `grpc` scheduler forwards actions to another server and has no workers.
/// The match is exhaustive so a new scheduler kind forces a decision here.
fn accepts_workers(spec: &SchedulerSpec) -> bool {
    match spec {
        SchedulerSpec::Simple(_) => true,
        SchedulerSpec::Grpc(_) => false,
        SchedulerSpec::CacheLookup(inner) => accepts_workers(&inner.scheduler),
        SchedulerSpec::PropertyModifier(inner) => accepts_workers(&inner.scheduler),
        SchedulerSpec::HistoricalResource(inner) => accepts_workers(&inner.scheduler),
    }
}

/// Recursively collects every `ref_store` name that appears anywhere inside a
/// single [`StoreSpec`] tree (the wrapper stores nest other `StoreSpec`s). The
/// match is exhaustive on purpose so a newly added wrapping store forces this
/// to be revisited.
fn collect_ref_store_names<'a>(spec: &'a StoreSpec, out: &mut Vec<&'a StoreRefName>) {
    match spec {
        StoreSpec::RefStore(ref_spec) => out.push(&ref_spec.name),
        StoreSpec::CacheMetrics(inner) => collect_ref_store_names(&inner.backend, out),
        StoreSpec::Verify(inner) => collect_ref_store_names(&inner.backend, out),
        StoreSpec::CompletenessChecking(inner) => {
            collect_ref_store_names(&inner.backend, out);
            collect_ref_store_names(&inner.cas_store, out);
        }
        StoreSpec::Compression(inner) => collect_ref_store_names(&inner.backend, out),
        StoreSpec::Dedup(inner) => {
            collect_ref_store_names(&inner.index_store, out);
            collect_ref_store_names(&inner.content_store, out);
        }
        StoreSpec::ExistenceCache(inner) => collect_ref_store_names(&inner.backend, out),
        StoreSpec::FastSlow(inner) => {
            collect_ref_store_names(&inner.fast, out);
            collect_ref_store_names(&inner.slow, out);
        }
        StoreSpec::Shard(inner) => {
            for shard in &inner.stores {
                collect_ref_store_names(&shard.store, out);
            }
        }
        StoreSpec::SizePartitioning(inner) => {
            collect_ref_store_names(&inner.lower_store, out);
            collect_ref_store_names(&inner.upper_store, out);
        }
        // Leaf stores and stores whose backends are concrete (non-`StoreSpec`)
        // specs carry no nested `ref_store` names.
        StoreSpec::Memory(_)
        | StoreSpec::ExperimentalCloudObjectStore(_)
        | StoreSpec::OntapS3ExistenceCache(_)
        | StoreSpec::Filesystem(_)
        | StoreSpec::Grpc(_)
        | StoreSpec::RedisStore(_)
        | StoreSpec::Noop(_)
        | StoreSpec::ExperimentalMongo(_) => {}
    }
}

/// Accumulates [`ConfigProblem`]s while walking a [`CasConfig`].
struct ReferenceChecker<'a> {
    store_names: HashSet<&'a str>,
    scheduler_names: HashSet<&'a str>,
    problems: Vec<ConfigProblem>,
}

impl<'a> ReferenceChecker<'a> {
    /// Records that `name` (found at `path`) must resolve to a declared store.
    fn store(&mut self, path: &str, name: &str) {
        if !self.store_names.contains(name) {
            self.problems.push(ConfigProblem::error(
                path,
                format!("references undefined store '{name}'"),
            ));
        }
    }

    /// Records that `name` (found at `path`) must resolve to a declared
    /// scheduler.
    fn scheduler(&mut self, path: &str, name: &str) {
        if !self.scheduler_names.contains(name) {
            self.problems.push(ConfigProblem::error(
                path,
                format!("references undefined scheduler '{name}'"),
            ));
        }
    }

    /// Walks a [`SchedulerSpec`] tree, resolving every store reference it
    /// carries and descending into nested schedulers. The match is exhaustive
    /// on purpose so a newly added scheduler variant forces this to be
    /// revisited (same design as [`collect_ref_store_names`]).
    fn scheduler_spec(&mut self, path: &str, spec: &'a SchedulerSpec) {
        match spec {
            SchedulerSpec::Simple(simple) => match &simple.experimental_backend {
                Some(ExperimentalSimpleSchedulerBackend::Redis(redis)) => {
                    self.store(
                        &format!("{path}.simple.experimental_backend.redis.redis_store"),
                        &redis.redis_store,
                    );
                }
                Some(ExperimentalSimpleSchedulerBackend::Memory) | None => {}
            },
            SchedulerSpec::CacheLookup(inner) => {
                self.store(&format!("{path}.cache_lookup.ac_store"), &inner.ac_store);
                self.scheduler_spec(&format!("{path}.cache_lookup.scheduler"), &inner.scheduler);
            }
            SchedulerSpec::PropertyModifier(inner) => {
                self.scheduler_spec(
                    &format!("{path}.property_modifier.scheduler"),
                    &inner.scheduler,
                );
            }
            SchedulerSpec::HistoricalResource(inner) => {
                // `snapshot_store` is deliberately not checked: by documented
                // contract a missing name only logs a warning at startup and
                // disables snapshot persistence, so flagging it here would
                // make `--check` stricter than boot.
                self.scheduler_spec(
                    &format!("{path}.historical_resource.scheduler"),
                    &inner.scheduler,
                );
            }
            // Carries no store references and no nested scheduler.
            SchedulerSpec::Grpc(_) => {}
        }
    }

    /// Resolves every `ref_store` name nested anywhere inside the declared
    /// store `name` against the declared store set. Each declared store is
    /// walked exactly once by the caller, so no visited set is needed and a
    /// `ref_store` cycle between declared stores is two resolved
    /// references.
    fn store_internal_refs(&mut self, name: &str, spec: &'a StoreSpec) {
        let mut refs: Vec<&'a StoreRefName> = Vec::new();
        collect_ref_store_names(spec, &mut refs);
        for ref_name in refs {
            self.store(&format!("stores[{name}]"), ref_name.as_str());
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing_test::traced_test;

    use super::*;

    #[test]
    fn single_use_worker_defaults_off_and_rejects_shared_processes() {
        assert!(!LocalWorkerConfig::default().single_use);
        let mut config = CasConfig::try_from_json5_str("{ stores: [], servers: [] }").unwrap();
        config.workers = Some(vec![WorkerConfig::Local(LocalWorkerConfig {
            single_use: true,
            ..Default::default()
        })]);
        config.validate_single_use_worker().unwrap();
        config
            .workers
            .as_mut()
            .unwrap()
            .push(WorkerConfig::Local(LocalWorkerConfig::default()));
        assert!(config.validate_single_use_worker().is_err());
        config.workers.as_mut().unwrap().pop();
        config.servers = serde_json5::from_str(
            r#"[{
            listener: { http: { socket_address: "127.0.0.1:50061" } },
            services: { health: { path: "/health" } }
        }]"#,
        )
        .unwrap();
        config.validate_single_use_worker().unwrap();
        config.servers[0].services.as_mut().unwrap().cas = Some(vec![]);
        assert!(config.validate_single_use_worker().is_err());
    }

    #[test]
    fn buck2_capture_requires_an_isolated_execution_container() {
        assert!(
            LocalWorkerConfig::default()
                .experimental_buck2_file_capture
                .is_none()
        );
        let capture = Buck2FileCaptureConfig {
            executable: "/shared/buck2-file-capture".to_string(),
            gateway: "http://capture/api-auth/v1/buck2-files".to_string(),
            token_file: "/capture-token/token".to_string(),
            state_directory: "/capture-state".to_string(),
            container_id: "pod-uid".to_string(),
            protected_paths: vec!["/worker-config".to_string()],
            internal_paths: vec!["/cas".to_string()],
            finalize_timeout_s: 120,
        };
        for (single_use, entrypoint, mount_namespace, valid) in [
            (true, "", None, true),
            (true, "", Some(false), true),
            (false, "", None, false),
            (true, "/outer-container-wrapper", None, false),
            (true, "", Some(true), false),
        ] {
            let mut config = CasConfig::try_from_json5_str("{ stores: [], servers: [] }").unwrap();
            config.workers = Some(vec![WorkerConfig::Local(LocalWorkerConfig {
                single_use,
                entrypoint: entrypoint.to_string(),
                use_mount_namespace: mount_namespace,
                experimental_buck2_file_capture: Some(capture.clone()),
                ..Default::default()
            })]);
            let result = config.validate_single_use_worker();
            assert_eq!(result.is_ok(), valid, "{result:?}");
        }
        for invalid in [
            "relative/helper",
            "/",
            "/./",
            "//",
            "/tmp/../token",
            "C:\\capture\\token",
            "/tmp/\0token",
        ] {
            let mut invalid_capture = capture.clone();
            invalid_capture.token_file = invalid.to_string();
            let mut config = CasConfig::try_from_json5_str("{ stores: [], servers: [] }").unwrap();
            config.workers = Some(vec![WorkerConfig::Local(LocalWorkerConfig {
                single_use: true,
                experimental_buck2_file_capture: Some(invalid_capture),
                ..Default::default()
            })]);
            assert!(
                config.validate_single_use_worker().is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    fn grpc_compression_configs(config: &CasConfig) -> Vec<(bool, Option<bool>)> {
        let mut configs = Vec::new();
        for store in &config.stores {
            store.spec.visit_grpc_specs(&mut |grpc| {
                configs.push((
                    matches!(grpc.store_type, StoreType::Cas),
                    grpc.experimental_remote_cache_compression,
                ));
            });
        }
        configs
    }

    #[test]
    fn capabilities_config_remote_cache_compression_deserializes_true() {
        let config: CapabilitiesConfig =
            serde_json5::from_str(r#"{"remote_cache_compression": true}"#).unwrap();

        assert!(config.remote_cache_compression);
    }

    #[test]
    fn capabilities_config_remote_cache_compression_defaults_false() {
        let config: CapabilitiesConfig = serde_json5::from_str("{}").unwrap();

        assert!(!config.remote_cache_compression);
    }

    #[test]
    fn omitted_grpc_compression_stays_disabled_without_zstd_intent() {
        let config = CasConfig::try_from_json5_str(
            r#"{
                stores: [{
                    name: "upstream",
                    grpc: {
                        endpoints: [{ address: "http://localhost:1234" }],
                        store_type: "cas",
                    },
                }],
                servers: [],
            }"#,
        )
        .unwrap();

        assert_eq!(grpc_compression_configs(&config), vec![(true, None)]);
    }

    #[test]
    fn explicit_grpc_zstd_intent_enables_other_eligible_grpc_stores() {
        let config = CasConfig::try_from_json5_str(
            r#"{
                stores: [
                    {
                        name: "enabled",
                        grpc: {
                            endpoints: [{ address: "http://localhost:1234" }],
                            store_type: "cas",
                            experimental_remote_cache_compression: true,
                        },
                    },
                    {
                        name: "inherited",
                        grpc: {
                            endpoints: [{ address: "http://localhost:5678" }],
                            store_type: "cas",
                        },
                    },
                ],
                servers: [],
            }"#,
        )
        .unwrap();

        assert_eq!(
            grpc_compression_configs(&config),
            vec![(true, Some(true)), (true, Some(true))]
        );
    }

    #[test]
    #[traced_test]
    fn ac_grpc_compression_warns_and_expresses_no_cas_intent() {
        let config = CasConfig::try_from_json5_str(
            r#"{
                stores: [
                    {
                        name: "action-cache",
                        grpc: {
                            instance_name: "ac",
                            endpoints: [{ address: "http://localhost:1234" }],
                            store_type: "ac",
                            experimental_remote_cache_compression: true,
                        },
                    },
                    {
                        name: "cas",
                        grpc: {
                            endpoints: [{ address: "http://localhost:5678" }],
                            store_type: "cas",
                        },
                    },
                ],
                servers: [],
            }"#,
        )
        .unwrap();

        // The AC-store setting is inert: it neither compresses AC RPCs nor
        // expresses process-wide zstd intent, so the CAS store stays unset.
        assert_eq!(
            grpc_compression_configs(&config),
            vec![(false, Some(true)), (true, None)]
        );
        assert!(logs_contain("store=\"action-cache\""));
        assert!(logs_contain("no effect"));
    }

    #[test]
    #[traced_test]
    fn capabilities_zstd_intent_defaults_nested_cas_grpc_and_warns_on_opt_out() {
        let config = CasConfig::try_from_json5_str(
            r#"{
                stores: [{
                    name: "nested-upstreams",
                    fast_slow: {
                        fast: {
                            grpc: {
                                instance_name: "auto",
                                endpoints: [{ address: "http://localhost:1234" }],
                                store_type: "cas",
                            },
                        },
                        slow: {
                            shard: {
                                stores: [
                                    {
                                        store: {
                                            grpc: {
                                                instance_name: "opt-out",
                                                endpoints: [{
                                                    address: "http://localhost:5678",
                                                }],
                                                store_type: "cas",
                                                experimental_remote_cache_compression: false,
                                            },
                                        },
                                        weight: 1,
                                    },
                                    {
                                        store: {
                                            grpc: {
                                                instance_name: "action-cache",
                                                endpoints: [{
                                                    address: "http://localhost:9012",
                                                }],
                                                store_type: "ac",
                                            },
                                        },
                                        weight: 1,
                                    },
                                ],
                            },
                        },
                    },
                }],
                servers: [{
                    listener: {
                        http: { socket_address: "127.0.0.1:50051" },
                    },
                    services: {
                        capabilities: [{
                            instance_name: "main",
                            remote_cache_compression: true,
                        }],
                    },
                }],
            }"#,
        )
        .unwrap();

        assert_eq!(
            grpc_compression_configs(&config),
            vec![(true, Some(true)), (true, Some(false)), (false, None),]
        );
        assert!(logs_contain("store=\"nested-upstreams\""));
        assert!(logs_contain("instance_name=\"opt-out\""));
        assert!(logs_contain(CasConfig::ZSTD_COMPRESSION_DOCS_URL));
    }

    #[test]
    fn action_cache_read_only_by_instance_reads_each_ac_service() {
        let mut config = CasConfig::try_from_json5_str("{ stores: [], servers: [] }").unwrap();
        // No Action Cache services: the map is empty.
        assert!(config.action_cache_read_only_by_instance().is_empty());

        config.servers = serde_json5::from_str(
            r#"[{
                listener: { http: { socket_address: "127.0.0.1:50051" } },
                services: { ac: [
                    { instance_name: "main", ac_store: "AC", read_only: false },
                    { instance_name: "pr", ac_store: "AC", read_only: true },
                ] }
            }]"#,
        )
        .unwrap();

        let by_instance = config.action_cache_read_only_by_instance();
        assert_eq!(by_instance.len(), 2);
        assert_eq!(by_instance.get("main"), Some(&false));
        assert_eq!(by_instance.get("pr"), Some(&true));
    }

    #[test]
    fn warns_only_for_shared_workers_and_a_writable_action_cache() {
        fn config(single_use: bool, read_only: bool) -> CasConfig {
            let mut config = CasConfig::try_from_json5_str("{ stores: [], servers: [] }").unwrap();
            config.workers = Some(vec![WorkerConfig::Local(LocalWorkerConfig {
                single_use,
                ..Default::default()
            })]);
            config.servers = serde_json5::from_str(&format!(
                r#"[{{
                    listener: {{ http: {{ socket_address: "127.0.0.1:50051" }} }},
                    services: {{ ac: [{{ ac_store: "AC", read_only: {read_only} }}] }}
                }}]"#
            ))
            .unwrap();
            config
        }

        // Unsafe: shared workers and an action cache that accepts client writes.
        assert!(config(false, false).shares_workers_with_writable_action_cache());
        // Safe: `single_use` workers isolate each job.
        assert!(!config(true, false).shares_workers_with_writable_action_cache());
        // Safe: the action cache rejects client writes.
        assert!(!config(false, true).shares_workers_with_writable_action_cache());

        // Safe: a cache-only process has no workers to share.
        let mut cache_only = config(false, false);
        cache_only.workers = None;
        assert!(!cache_only.shares_workers_with_writable_action_cache());
    }

    // ----------------------------------------------------------------------
    // `validate_references` (offline `nativelink --check`) tests.
    // ----------------------------------------------------------------------

    fn parse_cas_config(json5: &str) -> CasConfig {
        // The real load path, so the tests see the config startup would.
        CasConfig::try_from_json5_str(json5).expect("valid CasConfig json5")
    }

    /// A fully-resolved config that exercises worker, service and nested
    /// `ref_store` reference sites. `WRAPPED` wraps a `ref_store` to `CAS`,
    /// which the walk over every declared store must resolve.
    const VALID_CONFIG: &str = r#"{
        stores: [
            { name: "CAS", memory: {} },
            { name: "AC", memory: {} },
            { name: "WRAPPED", fast_slow: {
                fast: { ref_store: { name: "CAS" } },
                slow: { noop: {} },
            } },
        ],
        schedulers: [{ name: "SCHED", simple: {} }],
        workers: [{
            local: {
                worker_api_endpoint: { uri: "grpc://127.0.0.1:50061" },
                cas_fast_slow_store: "WRAPPED",
                upload_action_result: { ac_store: "AC" },
                work_directory: "/tmp/work",
                platform_properties: {},
            },
        }],
        servers: [{
            listener: { http: { socket_address: "0.0.0.0:50051" } },
            services: {
                cas: [{ instance_name: "main", cas_store: "CAS" }],
                ac: [{ instance_name: "main", ac_store: "AC" }],
                execution: [{ instance_name: "main", cas_store: "CAS", scheduler: "SCHED" }],
                capabilities: [{ instance_name: "main", remote_execution: { scheduler: "SCHED" } }],
                bytestream: [{ instance_name: "main", cas_store: "CAS" }],
                worker_api: { scheduler: "SCHED" },
            },
        }],
    }"#;

    #[test]
    fn validate_references_accepts_resolved_config() {
        parse_cas_config(VALID_CONFIG)
            .validate_references()
            .expect("every reference resolves");
    }

    #[test]
    fn validate_references_flags_undefined_service_store() {
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                servers: [{
                    listener: { http: { socket_address: "0.0.0.0:50051" } },
                    services: { cas: [{ instance_name: "main", cas_store: "CAS_TYPO" }] },
                }],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        assert_eq!(err.code, Code::InvalidArgument);
        let message = err.messages.join("\n");
        assert!(
            message.contains("CAS_TYPO"),
            "error must name the bad store ref: {message}"
        );
        assert!(
            message.contains("servers[0].services.cas[main].cas_store"),
            "error must name the reference site: {message}"
        );
    }

    #[test]
    fn validate_references_flags_worker_api_on_grpc_scheduler() {
        // A grpc scheduler, even behind a wrapper, builds no worker scheduler,
        // so startup cannot serve the worker API from it.
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                schedulers: [
                    { name: "REMOTE", grpc: { endpoint: { address: "grpc://127.0.0.1:50052" } } },
                    { name: "WRAPPED", property_modifier: {
                        modifications: [],
                        scheduler: { grpc: { endpoint: { address: "grpc://127.0.0.1:50052" } } },
                    } },
                    { name: "LOCAL", simple: {} },
                ],
                servers: [
                    {
                        listener: { http: { socket_address: "0.0.0.0:50061" } },
                        services: { worker_api: { scheduler: "REMOTE" } },
                    },
                    {
                        listener: { http: { socket_address: "0.0.0.0:50062" } },
                        services: { worker_api: { scheduler: "WRAPPED" } },
                    },
                    {
                        listener: { http: { socket_address: "0.0.0.0:50063" } },
                        services: { worker_api: { scheduler: "LOCAL" } },
                    },
                ],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        let message = err.messages.join("\n");
        assert!(
            message.contains("scheduler 'REMOTE', which forwards")
                && message.contains("scheduler 'WRAPPED', which forwards"),
            "error must name both schedulers that cannot accept workers: {message}"
        );
        assert!(
            !message.contains("'LOCAL'"),
            "a simple scheduler accepts workers: {message}"
        );
    }

    #[test]
    fn validate_references_accepts_worker_api_when_any_declaration_has_workers() {
        // Startup registers a worker scheduler only from declarations that
        // build one, so `S` serves the worker API from its `simple` entry
        // even though a later `grpc` entry reuses the name.
        let cfg = parse_cas_config(
            r#"{
                stores: [],
                schedulers: [
                    { name: "S", simple: {} },
                    { name: "S", grpc: { endpoint: { address: "grpc://127.0.0.1:50052" } } },
                ],
                servers: [{
                    listener: { http: { socket_address: "0.0.0.0:50061" } },
                    services: { worker_api: { scheduler: "S" } },
                }],
            }"#,
        );
        cfg.validate_references()
            .expect("a simple declaration under the name accepts workers");
    }

    #[test]
    fn validate_references_flags_server_without_services() {
        let cfg = parse_cas_config(
            r#"{
                stores: [],
                servers: [{ listener: { http: { socket_address: "0.0.0.0:50061" } } }],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        let message = err.messages.join("\n");
        assert!(
            message.contains("servers[0] has no 'services'"),
            "error must name the server: {message}"
        );
    }

    #[test]
    fn validate_references_flags_duplicate_worker_name() {
        // `worker_1` collides with the name startup gives the unnamed worker
        // at index 1, and `w` is used twice outright.
        let worker = |name: &str| {
            format!(
                r#"{{ local: {{ name: "{name}",
                    worker_api_endpoint: {{ uri: "grpc://127.0.0.1:1" }},
                    cas_fast_slow_store: "CAS", work_directory: "/tmp/w",
                    platform_properties: {{}} }} }}"#
            )
        };
        let cfg = parse_cas_config(&format!(
            r#"{{ stores: [{{ name: "CAS", memory: {{}} }}],
                workers: [{}, {}, {}, {}], servers: [] }}"#,
            worker("w"),
            worker(""),
            worker("worker_1"),
            worker("w"),
        ));
        let message = cfg.validate_references().unwrap_err().messages.join("\n");
        assert!(
            message.contains("workers[2] has the same name as workers[1]: 'worker_1'")
                && message.contains("workers[3] has the same name as workers[0]: 'w'"),
            "error must name both collisions: {message}"
        );
    }

    #[test]
    fn validate_references_checks_listener_addresses() {
        let server = |address: &str| {
            format!(
                r#"{{ listener: {{ http: {{ socket_address: "{address}" }} }},
                    services: {{ health: {{}} }} }}"#
            )
        };
        let cfg = parse_cas_config(&format!(
            "{{ stores: [], servers: [{}, {}, {}, {}, {}] }}",
            server("127.0.0.1:50051"),
            server("127.0.0.1:50051"),
            server("not-an-address"),
            server("127.0.0.1:0"),
            server("127.0.0.1:0"),
        ));
        let message = cfg.validate_references().unwrap_err().messages.join("\n");
        assert!(
            message.contains(
                "servers[1] listens on the same address as servers[0]: '127.0.0.1:50051'"
            ),
            "error must report the shared address: {message}"
        );
        assert!(
            message.contains("servers[2].listener.http.socket_address is not a valid address"),
            "error must report the bad address: {message}"
        );
        assert!(
            !message.contains("servers[4]"),
            "port 0 picks a free port, so it can repeat: {message}"
        );
    }

    #[test]
    fn validate_references_labels_empty_instance_name() {
        let cfg = parse_cas_config(
            r#"{
                stores: [],
                servers: [{
                    listener: { http: { socket_address: "0.0.0.0:50051" } },
                    services: { cas: [{ cas_store: "MISSING" }] },
                }],
            }"#,
        );
        let message = cfg.validate_references().unwrap_err().messages.join("\n");
        assert!(
            message.contains(r#"servers[0].services.cas[""].cas_store"#),
            "an empty instance name must render as \"\": {message}"
        );
    }

    #[test]
    fn validate_references_flags_undefined_scheduler() {
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                schedulers: [{ name: "SCHED", simple: {} }],
                servers: [{
                    listener: { http: { socket_address: "0.0.0.0:50051" } },
                    services: {
                        execution: [{
                            instance_name: "main",
                            cas_store: "CAS",
                            scheduler: "SCHED_TYPO",
                        }],
                    },
                }],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        assert_eq!(err.code, Code::InvalidArgument);
        let message = err.messages.join("\n");
        assert!(
            message.contains("SCHED_TYPO"),
            "error must name the bad scheduler ref: {message}"
        );
        assert!(
            message.contains("servers[0].services.execution[main].scheduler"),
            "error must name the reference site: {message}"
        );
    }

    #[test]
    fn validate_references_flags_undefined_worker_store() {
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                workers: [{
                    local: {
                        worker_api_endpoint: { uri: "grpc://127.0.0.1:50061" },
                        cas_fast_slow_store: "WORKER_TYPO",
                        work_directory: "/tmp/work",
                        platform_properties: {},
                    },
                }],
                servers: [],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        let message = err.messages.join("\n");
        assert!(
            message.contains("WORKER_TYPO")
                && message.contains("workers[0].local.cas_fast_slow_store"),
            "error must name the bad worker store ref and site: {message}"
        );
    }

    #[test]
    fn validate_references_flags_nested_ref_store_typo() {
        // The walk over every declared store must find the `ref_store` typo
        // nested inside `WRAPPED`.
        let cfg = parse_cas_config(
            r#"{
                stores: [
                    { name: "CAS", memory: {} },
                    { name: "WRAPPED", fast_slow: {
                        fast: { ref_store: { name: "CAS_TYPO" } },
                        slow: { noop: {} },
                    } },
                ],
                servers: [{
                    listener: { http: { socket_address: "0.0.0.0:50051" } },
                    services: { cas: [{ instance_name: "main", cas_store: "WRAPPED" }] },
                }],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        let message = err.messages.join("\n");
        assert!(
            message.contains("CAS_TYPO") && message.contains("stores[WRAPPED]"),
            "error must name the nested bad ref and its owning store: {message}"
        );
    }

    #[test]
    fn validate_references_flags_undefined_scheduler_redis_store() {
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                schedulers: [{
                    name: "SCHED",
                    simple: {
                        experimental_backend: {
                            redis: { redis_store: "REDIS_TYPO" },
                        },
                    },
                }],
                servers: [],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        let message = err.messages.join("\n");
        assert!(
            message.contains("REDIS_TYPO")
                && message
                    .contains("schedulers[SCHED].simple.experimental_backend.redis.redis_store"),
            "error must name the bad redis store ref and its site: {message}"
        );
    }

    #[test]
    fn validate_references_flags_undefined_cache_lookup_ac_store() {
        // The nested scheduler inside `cache_lookup` must be walked too,
        // and its `ac_store` reference resolved.
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                schedulers: [{
                    name: "SCHED",
                    property_modifier: {
                        modifications: [],
                        scheduler: {
                            cache_lookup: {
                                ac_store: "AC_TYPO",
                                scheduler: { simple: {} },
                            },
                        },
                    },
                }],
                servers: [],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        let message = err.messages.join("\n");
        assert!(
            message.contains("AC_TYPO")
                && message.contains(
                    "schedulers[SCHED].property_modifier.scheduler.cache_lookup.ac_store"
                ),
            "error must name the nested bad ac_store ref and its site: {message}"
        );
    }

    #[test]
    fn validate_references_flags_undefined_chunking_index_store() {
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                servers: [{
                    listener: { http: { socket_address: "0.0.0.0:50051" } },
                    services: {
                        cas: [{
                            instance_name: "main",
                            cas_store: "CAS",
                            experimental_chunking: { index_store: "INDEX_TYPO" },
                        }],
                    },
                }],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        let message = err.messages.join("\n");
        assert!(
            message.contains("INDEX_TYPO")
                && message
                    .contains("servers[0].services.cas[main].experimental_chunking.index_store"),
            "error must name the bad index store ref and its site: {message}"
        );
    }

    #[test]
    fn validate_references_flags_undefined_fetch_downloader_cas_store() {
        // The HTTP downloader of a fetch service writes into its own CAS
        // reference, separate from `fetch_store`.
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "FETCH", memory: {} }],
                servers: [{
                    listener: { http: { socket_address: "0.0.0.0:50051" } },
                    services: {
                        fetch: [{
                            instance_name: "main",
                            fetch_store: "FETCH",
                            downloader: { cas_store: "MISSING_CAS" },
                        }],
                    },
                }],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        assert_eq!(err.code, Code::InvalidArgument);
        assert!(
            err.to_string().contains(
                "servers[0].services.fetch[main].downloader.cas_store references undefined store 'MISSING_CAS'"
            ),
            "{err}"
        );
    }

    #[test]
    fn validate_references_flags_duplicate_store_name() {
        let cfg = parse_cas_config(
            r#"{
                stores: [
                    { name: "CAS", memory: {} },
                    { name: "CAS", memory: {} },
                ],
                servers: [],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        let message = err.messages.join("\n");
        assert!(
            message.contains("duplicate store name 'CAS'"),
            "error must report the duplicate store: {message}"
        );
    }

    #[test]
    fn validate_references_accepts_duplicate_scheduler_name() {
        // Startup accepts a repeated scheduler name (the last one wins), so
        // validation must too; it is surfaced as a warning instead.
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                schedulers: [
                    { name: "SCHED", simple: {} },
                    { name: "OTHER", simple: {} },
                    { name: "SCHED", simple: {} },
                    { name: "SCHED", simple: {} },
                ],
                servers: [],
            }"#,
        );
        cfg.validate_references()
            .expect("duplicate scheduler names must not fail validation");
        assert_eq!(
            cfg.problems(),
            vec![ConfigProblem::warning(
                "",
                "scheduler name 'SCHED' is declared more than once; later definitions \
                 replace earlier ones",
            )],
        );
    }

    #[test]
    fn problems_split_path_from_message() {
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }, { name: "CAS", memory: {} }],
                servers: [{
                    listener: { http: { socket_address: "127.0.0.1:50051" } },
                    services: { ac: [{ instance_name: "main", ac_store: "AC" }] },
                }],
            }"#,
        );
        let problems = cfg.problems();
        assert_eq!(
            problems,
            vec![
                ConfigProblem::error("", "duplicate store name 'CAS'"),
                ConfigProblem::error(
                    "servers[0].services.ac[main].ac_store",
                    "references undefined store 'AC'",
                ),
            ],
        );
        let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
        assert_eq!(
            lines,
            [
                "duplicate store name 'CAS'",
                "servers[0].services.ac[main].ac_store references undefined store 'AC'",
            ],
        );
    }

    #[test]
    fn problems_empty_without_duplicate_schedulers() {
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                schedulers: [
                    { name: "A", simple: {} },
                    { name: "B", simple: {} },
                ],
                servers: [],
            }"#,
        );
        assert!(cfg.problems().is_empty());
    }

    #[test]
    fn validate_references_collects_all_problems() {
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                schedulers: [{ name: "SCHED", simple: {} }],
                servers: [{
                    listener: { http: { socket_address: "0.0.0.0:50051" } },
                    services: {
                        cas: [{ instance_name: "main", cas_store: "BAD_CAS" }],
                        ac: [{ instance_name: "main", ac_store: "BAD_AC" }],
                    },
                }],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        let message = err.messages.join("\n");
        // Reporting is not fail-fast: both bad references appear together.
        assert!(
            message.contains("BAD_CAS") && message.contains("BAD_AC"),
            "error must collect every unresolved reference: {message}"
        );
    }

    #[test]
    fn validate_references_flags_references_into_empty_store_registry() {
        // `stores: []` is a complete declaration of zero stores, not a
        // fragment: startup has no other source of definitions, so every
        // service reference below fails at boot and must fail here too.
        let cfg = parse_cas_config(
            r#"{
                stores: [],
                servers: [{
                    listener: { http: { socket_address: "0.0.0.0:50051" } },
                    services: {
                        cas: [{ instance_name: "", cas_store: "CAS_MAIN_STORE" }],
                        execution: [{
                            instance_name: "",
                            cas_store: "WORKER_STORE",
                            scheduler: "MAIN_SCHEDULER",
                        }],
                    },
                }],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        assert_eq!(err.code, Code::InvalidArgument);
        let message = err.messages.join("\n");
        assert!(message.contains(
            r#"servers[0].services.cas[""].cas_store references undefined store 'CAS_MAIN_STORE'"#
        ));
        assert!(message.contains(
            r#"servers[0].services.execution[""].cas_store references undefined store 'WORKER_STORE'"#
        ));
        // `schedulers` is absent entirely; that is an empty registry too.
        assert!(message.contains(
            r#"servers[0].services.execution[""].scheduler references undefined scheduler 'MAIN_SCHEDULER'"#
        ));
    }

    #[test]
    fn validate_references_flags_scheduler_reference_with_no_schedulers_declared() {
        // A worker API service pointing at a scheduler when the config
        // declares none fails at startup; `--check` must agree.
        let cfg = parse_cas_config(
            r#"{
                stores: [{ name: "CAS", memory: {} }],
                servers: [{
                    listener: { http: { socket_address: "0.0.0.0:50061" } },
                    services: { worker_api: { scheduler: "MISSING" } },
                }],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        assert_eq!(err.code, Code::InvalidArgument);
        assert!(err.to_string().contains(
            "servers[0].services.worker_api.scheduler references undefined scheduler 'MISSING'"
        ));
    }

    #[test]
    fn validate_references_flags_unreferenced_store_internal_refs() {
        // Startup constructs every declared store and runs `post_init` on
        // all of them, so a dangling `ref_store` inside a store that no
        // service or worker uses still fails to boot. The walker must cover
        // every declared store, not just the reachable ones.
        let cfg = parse_cas_config(
            r#"{
                stores: [
                    { name: "REAL", memory: {} },
                    { name: "UNUSED", ref_store: { name: "MISSING" } },
                ],
                servers: [],
            }"#,
        );
        let err = cfg.validate_references().unwrap_err();
        assert_eq!(err.code, Code::InvalidArgument);
        assert!(
            err.to_string()
                .contains("stores[UNUSED] references undefined store 'MISSING'"),
            "{err}"
        );
    }

    #[test]
    fn validate_references_accepts_ref_store_cycle_between_declared_stores() {
        // Every declared store is walked once, so a cycle is two resolved
        // references rather than an infinite walk.
        let cfg = parse_cas_config(
            r#"{
                stores: [
                    { name: "A", ref_store: { name: "B" } },
                    { name: "B", ref_store: { name: "A" } },
                ],
                servers: [],
            }"#,
        );
        cfg.validate_references()
            .expect("a ref_store cycle resolves without looping");
    }
}
