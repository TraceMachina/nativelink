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

use std::collections::HashMap;

use nativelink_error::{Error, ResultExt};
use serde::{Deserialize, Serialize};

use crate::schedulers::SchedulerSpec;
use crate::serde_utils::{
    convert_data_size_with_shellexpand, convert_duration_with_shellexpand,
    convert_numeric_with_shellexpand, convert_optional_numeric_with_shellexpand,
    convert_optional_string_with_shellexpand, convert_string_with_shellexpand,
    convert_vec_string_with_shellexpand,
};
use crate::stores::{ClientTlsConfig, ConfigDigestHashFunction, StoreRefName, StoreSpec};

/// Name of the scheduler. This type will be used when referencing a
/// scheduler in the `CasConfig::schedulers`'s map key.
pub type SchedulerRefName = String;

/// Used when the config references `instance_name` in the protocol.
pub type InstanceName = String;

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WithInstanceName<T> {
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
pub struct NamedConfig<Spec> {
    pub name: String,
    #[serde(flatten)]
    pub spec: Spec,
}

#[derive(Deserialize, Serialize, Debug, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
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

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct CasStoreConfig {
    /// The store name referenced in the `stores` map in the main config.
    /// This store name referenced here may be reused multiple times.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub cas_store: StoreRefName,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesRemoteExecutionConfig {
    /// Scheduler used to configure the capabilities of remote execution.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub scheduler: SchedulerRefName,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesConfig {
    /// Configuration for remote execution capabilities.
    /// If not set the capabilities service will inform the client that remote
    /// execution is not supported.
    pub remote_execution: Option<CapabilitiesRemoteExecutionConfig>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ExecutionConfig {
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
pub struct FetchConfig {
    /// The store name referenced in the `stores` map in the main config.
    /// This store name referenced here may be reused multiple times.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub fetch_store: StoreRefName,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
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
fn default<T: Default + PartialEq>(t: &T) -> bool {
    *t == Default::default()
}

#[derive(Deserialize, Serialize, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
        skip_serializing_if = "default"
    )]
    pub max_bytes_per_stream: usize,

    /// In the event a client disconnects while uploading a blob, we will hold
    /// the internal stream open for this many seconds before closing it.
    /// This allows clients that disconnect to reconnect and continue uploading
    /// the same blob.
    ///
    /// Default: 10 (seconds)
    #[serde(
        default,
        deserialize_with = "convert_duration_with_shellexpand",
        skip_serializing_if = "default"
    )]
    pub persist_stream_on_disconnect_timeout: usize,
}

// Older bytestream config. All fields are as per the newer docs, but this requires
// the hashed cas_stores v.s. the WithInstanceName approach. This should _not_ be updated
// with newer fields, and eventually dropped
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct OldByteStreamConfig {
    pub cas_stores: HashMap<InstanceName, StoreRefName>,
    #[serde(
        default,
        deserialize_with = "convert_data_size_with_shellexpand",
        skip_serializing_if = "default"
    )]
    pub max_bytes_per_stream: usize,
    #[serde(
        default,
        deserialize_with = "convert_data_size_with_shellexpand",
        skip_serializing_if = "default"
    )]
    pub max_decoding_message_size: usize,
    #[serde(
        default,
        deserialize_with = "convert_duration_with_shellexpand",
        skip_serializing_if = "default"
    )]
    pub persist_stream_on_disconnect_timeout: usize,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct WorkerApiConfig {
    /// The scheduler name referenced in the `schedulers` map in the main config.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub scheduler: SchedulerRefName,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
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
pub struct HealthConfig {
    /// Path to register the health status check. If path is "/status", and your
    /// domain is "example.com", you can reach the endpoint with:
    /// <http://example.com/status>.
    ///
    /// Default: "/status"
    #[serde(default)]
    pub path: String,

    // Timeout on health checks. Defaults to 5s.
    #[serde(default)]
    pub timeout_seconds: u64,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct BepConfig {
    /// The store to publish build events to.
    /// The store name referenced in the `stores` map in the main config.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub store: StoreRefName,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
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
pub struct OriginEventsPublisherSpec {
    /// The store to publish nativelink events to.
    /// The store name referenced in the `stores` map in the main config.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub store: StoreRefName,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
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
    /// features and versions of this bazel GRPC service.
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

    /// Note: This is in seconds.
    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    pub experimental_http2_keep_alive_timeout: Option<u32>,

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
pub enum ListenerConfig {
    /// Listener for HTTP/HTTPS/HTTP2 sockets.
    Http(HttpListener),
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct HttpListener {
    /// Address to listen on. Example: `127.0.0.1:8080` or `:8080` to listen
    /// to all IPs.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub socket_address: String,

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

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum WorkerProperty {
    /// List of static values.
    /// Note: Generally there should only ever be 1 value, but if the platform
    /// property key is `PropertyType::Priority` it may have more than one value.
    #[serde(deserialize_with = "convert_vec_string_with_shellexpand")]
    Values(Vec<String>),

    /// A dynamic configuration. The string will be executed as a command
    /// (not sell) and will be split by "\n" (new line character).
    QueryCmd(String),
}

/// Generic config for an endpoint and associated configs.
#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct EndpointConfig {
    /// URI of the endpoint.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub uri: String,

    /// Timeout in seconds that a request should take.
    /// Default: 5 (seconds)
    pub timeout: Option<f32>,

    /// The TLS configuration to use to connect to the endpoint.
    pub tls_config: Option<ClientTlsConfig>,
}

#[derive(Copy, Clone, Deserialize, Serialize, Debug, Default)]
#[serde(rename_all = "snake_case")]
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
pub enum EnvironmentSource {
    /// The name of the platform property in the action to get the value from.
    Property(String),

    /// The raw value to set.
    Value(#[serde(deserialize_with = "convert_string_with_shellexpand")] String),

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
    /// Default: `UploadCacheResultsStrategy::SuccessOnly`
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
    /// Default: `UploadCacheResultsStrategy::FailuresOnly`
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

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct LocalWorkerConfig {
    /// Name of the worker. This is give a more friendly name to a worker for logging
    /// and metric publishing. This is also the prefix of the worker id
    /// (ie: "{name}{uuidv6}").
    /// Default: {Index position in the workers list}
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub name: String,

    /// Endpoint which the worker will connect to the scheduler's `WorkerApiService`.
    pub worker_api_endpoint: EndpointConfig,

    /// The maximum time an action is allowed to run. If a task requests for a timeout
    /// longer than this time limit, the task will be rejected. Value in seconds.
    ///
    /// Default: 1200 (seconds / 20 mins)
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub max_action_timeout: usize,

    /// Maximum number of inflight tasks this worker can cope with.
    ///
    /// Default: 0 (infinite tasks)
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_inflight_tasks: u64,

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

    /// An optional mapping of environment names to set for the execution
    /// as well as those specified in the action itself.  If set, will set each
    /// key as an environment variable before executing the job with the value
    /// of the environment variable being the value of the property of the
    /// action being executed of that name or the fixed value.
    pub additional_environment: Option<HashMap<String, EnvironmentSource>>,

    /// Optional directory cache configuration for improving performance by caching
    /// reconstructed input directories and using hardlinks instead of rebuilding
    /// them from CAS for every action.
    /// Default: None (directory cache disabled)
    pub directory_cache: Option<DirectoryCacheConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
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
}

const fn default_directory_cache_max_entries() -> usize {
    1000
}

const fn default_directory_cache_max_size_bytes() -> u64 {
    10 * 1024 * 1024 * 1024 // 10 GB
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum WorkerConfig {
    /// A worker type that executes jobs locally on this machine.
    Local(LocalWorkerConfig),
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
#[serde(deny_unknown_fields)]
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
    /// # Errors
    ///
    /// Will return `Err` if we can't load the file.
    pub fn try_from_json5_file(config_file: &str) -> Result<Self, Error> {
        let json_contents = std::fs::read_to_string(config_file)
            .err_tip(|| format!("Could not open config file {config_file}"))?;
        Ok(serde_json5::from_str(&json_contents)?)
    }
}
