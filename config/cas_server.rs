// Copyright 2022 The Turbo Cache Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;

use serde::Deserialize;

use serde_utils::{convert_numeric_with_shellexpand, convert_string_with_shellexpand};
use stores::StoreRefName;

/// Name of the scheduler. This type will be used when referencing a
/// scheduler in the `CasConfig::schedulers`'s map key.
pub type SchedulerRefName = String;

/// Used when the config references `instance_name` in the protocol.
pub type InstanceName = String;

#[derive(Deserialize, Debug, Default, Clone, Copy)]
pub enum CompressionAlgorithm {
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
#[derive(Deserialize, Debug, Default)]
pub struct CompressionConfig {
    /// The compression algorithm that the server will use when sending
    /// responses to clients. Enabling this will likely save a lot of
    /// data transfer, but will consume a lot of CPU and add a lot of
    /// latency.
    /// see: https://github.com/allada/turbo-cache/issues/109
    ///
    /// Default: CompressionAlgorithm::None
    pub send_compression_algorithm: Option<CompressionAlgorithm>,

    /// The compression algorithm that the server will accept from clients.
    /// The server will broadcast the supported compression algorithms to
    /// clients and the client will choose which compression algorithm to
    /// use. Enabling this will likely save a lot of data transfer, but
    /// will consume a lot of CPU and add a lot of latency.
    /// see: https://github.com/allada/turbo-cache/issues/109
    ///
    /// Defaults: <no supported compression>
    pub accepted_compression_algorithms: Vec<CompressionAlgorithm>,
}

#[derive(Deserialize, Debug)]
pub struct AcStoreConfig {
    /// The store name referenced in the `stores` map in the main config.
    /// This store name referenced here may be reused multiple times.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub ac_store: StoreRefName,
}

#[derive(Deserialize, Debug)]
pub struct CasStoreConfig {
    /// The store name referenced in the `stores` map in the main config.
    /// This store name referenced here may be reused multiple times.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub cas_store: StoreRefName,
}

#[derive(Deserialize, Debug, Default)]
pub struct CapabilitiesRemoteExecutionConfig {
    /// Scheduler used to configure the capabilities of remote execution.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub scheduler: SchedulerRefName,
}

#[derive(Deserialize, Debug, Default)]
pub struct CapabilitiesConfig {
    /// Configuration for remote execution capabilities.
    /// If not set the capabilities service will inform the client that remote
    /// execution is not supported.
    pub remote_execution: Option<CapabilitiesRemoteExecutionConfig>,
}

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
pub struct ByteStreamConfig {
    /// Name of the store in the "stores" configuration.
    pub cas_stores: HashMap<InstanceName, StoreRefName>,

    // Max number of bytes to send on each grpc stream chunk.
    #[serde(deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_bytes_per_stream: usize,
}

#[derive(Deserialize, Debug)]
pub struct WorkerApiConfig {
    /// The scheduler name referenced in the `schedulers` map in the main config.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub scheduler: SchedulerRefName,
}

#[derive(Deserialize, Debug, Default)]
pub struct PrometheusConfig {
    /// Path to register prometheus metrics. If path is "/metrics", and your
    /// domain is "example.com", you can reach the endpoint with:
    /// http://example.com/metrics.
    ///
    /// Default: "/metrics"
    #[serde(default)]
    pub path: String,
}

#[derive(Deserialize, Debug)]
pub struct ServicesConfig {
    /// The Content Addressable Storage (CAS) backend config.
    /// The key is the instance_name used in the protocol and the
    /// value is the underlying CAS store config.
    pub cas: Option<HashMap<InstanceName, CasStoreConfig>>,

    /// The Action Cache (AC) backend config.
    /// The key is the instance_name used in the protocol and the
    /// value is the underlying AC store config.
    pub ac: Option<HashMap<InstanceName, AcStoreConfig>>,

    /// Capabilities service is required in order to use most of the
    /// bazel protocol. This service is used to provide the supported
    /// features and versions of this bazel GRPC service.
    pub capabilities: Option<HashMap<InstanceName, CapabilitiesConfig>>,

    /// The remote execution service configuration.
    /// NOTE: This service is under development and is currently just a
    /// place holder.
    pub execution: Option<HashMap<InstanceName, ExecutionConfig>>,

    /// This is the service used to stream data to and from the CAS.
    /// Bazel's protocol strongly encourages users to use this streaming
    /// interface to interact with the CAS when the data is large.
    pub bytestream: Option<ByteStreamConfig>,

    /// This is the service used for workers to connect and communicate
    /// through.
    /// NOTE: This service should be served on a different, non-public port.
    /// In other words, `worker_api` configuration should not have any other
    /// services that are served on the same port. Doing so is a security
    /// risk, as workers have a different permission set than a client
    /// that makes the remote execution/cache requests.
    pub worker_api: Option<WorkerApiConfig>,

    /// Prometheus metrics configuration. Metrics are gathered as a singleton
    /// but may be served on multiple endpoints.
    pub prometheus: Option<PrometheusConfig>,
}

#[derive(Deserialize, Debug)]
pub struct ServerConfig {
    /// Address to listen on. Example: `127.0.0.1:8080` or `:8080` to listen
    /// to all IPs.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub listen_address: String,

    /// Data transport compression configuration to use for this service.
    #[serde(default)]
    pub compression: CompressionConfig,

    /// Services to attach to server.
    pub services: Option<ServicesConfig>,
}

#[allow(non_camel_case_types)]
#[derive(Deserialize, Debug)]
pub enum WrokerProperty {
    /// List of static values.
    /// Note: Generally there should only ever be 1 value, but if the platform
    /// property key is PropertyType::Priority it may have more than one value.
    values(Vec<String>),

    /// A dynamic configuration. The string will be executed as a command
    /// (not sell) and will be split by "\n" (new line character).
    query_cmd(String),
}

/// Generic config for an endpoint and associated configs.
#[derive(Deserialize, Debug, Default)]
pub struct EndpointConfig {
    /// URI of the endpoint.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub uri: String,

    /// Timeout in seconds that a request should take.
    /// Default: 5 (seconds)
    pub timeout: Option<f32>,
}

#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Deserialize, Debug, Default)]
pub enum UploadCacheResultsStrategy {
    /// Only upload action results with an exit code of 0.
    #[default]
    SuccessOnly,
    /// Don't upload any action results.
    Never,
    /// Upload all action results that complete.
    Everything,
}

#[derive(Deserialize, Debug, Default)]
pub struct LocalWorkerConfig {
    /// Name of the worker. This is give a more friendly name to a worker for logging
    /// and metric publishing.
    /// Default: <Index position in the workers list>.
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub name: String,

    /// Endpoint which the worker will connect to the scheduler's WorkerApiService.
    pub worker_api_endpoint: EndpointConfig,

    /// The command to execute on every execution request. This will be parsed as
    /// a command + arguments (not shell).
    /// Example: "run.sh" and a job with command: "sleep 5" will result in a
    /// command like: "run.sh sleep 5".
    /// Default: <Use the command from the job request>.
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub entrypoint_cmd: String,

    /// Underlying CAS store that the worker will use to download CAS artifacts.
    /// This store must be a `FastSlowStore`. The `fast` store must be a
    /// `FileSystemStore` because it will use hardlinks when building out the files
    /// instead of copying the files. The slow store must eventually resolve to the
    /// same store the scheduler/client uses to send job requests.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub cas_fast_slow_store: StoreRefName,

    /// Underlying AC store that the worker will use to publish execution results
    /// into. Objects placed in this store should be reachable from the
    /// scheduler/client-cas after they have finished updating.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub ac_store: StoreRefName,

    /// In which situations should the results be published to the ac_store, if
    /// set to SuccessOnly then only results with an exit code of 0 will be
    /// uploaded, if set to Everything all completed results will be uploaded.
    #[serde(default)]
    pub ac_store_strategy: UploadCacheResultsStrategy,

    /// The directory work jobs will be executed from. This directory will be fully
    /// managed by the worker service and will be purged on startup.
    /// This directory and the directory referenced in local_filesystem_store_ref's
    /// stores::FilesystemStore::content_path must be on the same filesystem.
    /// Hardlinks will be used when placing files that are accessible to the jobs
    /// that are sourced from local_filesystem_store_ref's content_path.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub work_directory: String,

    /// Properties of this worker. This configuration will be sent to the scheduler
    /// and used to tell the scheduler to restrict what should be executed on this
    /// worker.
    pub platform_properties: HashMap<String, WrokerProperty>,

    /// An optional script to run before every action is processed on the worker.
    /// The value should be the full path to the script to execute and will pause
    /// all actions on the worker if it returns an exit code other than 0.
    /// If not set, then the worker will never pause and will continue to accept
    /// jobs according to the scheduler configuration.
    /// This is useful, for example, if the worker should not take any more
    /// actions until there is enough resource available on the machine to
    /// handle them.
    pub precondition_script: Option<String>,
}

#[allow(non_camel_case_types)]
#[derive(Deserialize, Debug)]
pub enum WorkerConfig {
    /// A worker type that executes jobs locally on this machine.
    local(LocalWorkerConfig),
}

#[derive(Deserialize, Debug, Clone, Copy)]
pub struct GlobalConfig {
    /// Maximum number of open files that can be opened at one time.
    /// This value is not strictly enforced, it is a best effort. Some internal libraries
    /// open files or read metadata from a files which do not obay this limit, however
    /// the vast majority of cases will have this limit be honored.
    /// As a rule of thumb this value should be less than half the value of `ulimit -n`.
    /// Any network open file descriptors is not counted in this limit, but is counted
    /// in the kernel limit. It is a good idea to set a very large `ulimit -n`.
    /// Note: This value must be greater than 10.
    ///
    /// Default: 512
    #[serde(deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_open_files: usize,

    /// This flag can be used to prevent metrics from being collected at runtime.
    /// Metrics are still able to be collected, but this flag prevents metrics that
    /// are collected at runtime (performance metrics) from being tallied. The
    /// overhead of collecting metrics is very low, so this flag should only be
    /// used if there is a very good reason to disable metrics.
    /// This flag can be forcably set using the `TURBO_CACHE_DISABLE_METRICS` variable.
    /// If the variable is set it will always disable metrics regardless of what
    /// this flag is set to.
    ///
    /// Default: <true (disabled) if no prometheus service enabled, false otherwise>
    #[serde(default)]
    pub disable_metrics: bool,
}

#[derive(Deserialize, Debug)]
pub struct CasConfig {
    /// List of stores available to use in this config.
    /// The keys can be used in other configs when needing to reference a store.
    pub stores: HashMap<StoreRefName, stores::StoreConfig>,

    /// Worker configurations used to execute jobs.
    pub workers: Option<Vec<WorkerConfig>>,

    /// List of schedulers available to use in this config.
    /// The keys can be used in other configs when needing to reference a
    /// scheduler.
    pub schedulers: Option<HashMap<SchedulerRefName, schedulers::SchedulerConfig>>,

    /// Servers to setup for this process.
    pub servers: Vec<ServerConfig>,

    /// Any global configurations that apply to all modules live here.
    pub global: Option<GlobalConfig>,
}
