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

use core::cmp::Ordering;
use core::convert::Into;
use core::fmt::Display;
use core::hash::Hash;
use core::time::Duration;
use std::collections::HashMap;
use std::time::SystemTime;

use humantime::format_duration;
use nativelink_error::{Error, ResultExt, error_if, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, publish,
};
use nativelink_proto::build::bazel::remote::execution::v2::{
    Action, ActionResult as ProtoActionResult, ExecuteOperationMetadata, ExecuteRequest,
    ExecuteResponse, ExecutedActionMetadata, FileNode, LogFile, OutputDirectory, OutputFile,
    OutputSymlink, SymlinkNode, execution_stage,
};
use nativelink_proto::google::longrunning::Operation;
use nativelink_proto::google::longrunning::operation::Result as LongRunningResult;
use nativelink_proto::google::rpc::Status;
use prost::Message;
use prost::bytes::Bytes;
use prost_types::Any;
use serde::ser::Error as SerdeError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::common::{DigestInfo, HashMapExt, VecExt};
use crate::digest_hasher::DigestHasherFunc;

/// Default priority remote execution jobs will get when not provided.
pub const DEFAULT_EXECUTION_PRIORITY: i32 = 0;

/// Exit code sent if there is an internal error.
pub const INTERNAL_ERROR_EXIT_CODE: i32 = -178;

/// Holds an id that is unique to the client for a requested operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OperationId {
    Uuid(Uuid),
    String(String),
}

impl OperationId {
    pub fn into_string(self) -> String {
        match self {
            Self::Uuid(uuid) => uuid.to_string(),
            Self::String(name) => name,
        }
    }
}

impl Default for OperationId {
    fn default() -> Self {
        Self::Uuid(Uuid::new_v4())
    }
}

impl Display for OperationId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Uuid(uuid) => uuid.fmt(f),
            Self::String(name) => f.write_str(name),
        }
    }
}

impl MetricsComponent for OperationId {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::String(self.to_string()))
    }
}

impl From<&str> for OperationId {
    fn from(value: &str) -> Self {
        Uuid::parse_str(value).map_or_else(|_| Self::String(value.to_string()), Self::Uuid)
    }
}

impl From<String> for OperationId {
    fn from(value: String) -> Self {
        Uuid::parse_str(&value).map_or(Self::String(value), Self::Uuid)
    }
}

impl TryFrom<Bytes> for OperationId {
    type Error = Error;

    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        // This is an optimized path to attempt to do the conversion in-place
        // to avoid an extra allocation/copy.
        match value.try_into_mut() {
            // We are the only reference to the Bytes, so we can convert it into a Vec<u8>
            // for free then convert the Vec<u8> to a String for free too.
            Ok(value) => {
                let value = String::from_utf8(value.into()).map_err(|e| {
                    make_input_err!(
                        "Failed to convert bytes to string in try_from<Bytes> for OperationId : {e:?}"
                    )
                })?;
                Ok(Self::from(value))
            }
            // We could not take ownership of the Bytes, so we may need to copy our data.
            Err(value) => {
                let value = core::str::from_utf8(&value).map_err(|e| {
                    make_input_err!(
                        "Failed to convert bytes to string in try_from<Bytes> for OperationId : {e:?}"
                    )
                })?;
                Ok(Self::from(value))
            }
        }
    }
}

/// Unique id of worker.
#[derive(Default, Eq, PartialEq, Hash, Clone, Serialize, Deserialize)]
pub struct WorkerId(pub String);

impl MetricsComponent for WorkerId {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::String(self.0.clone()))
    }
}

impl Display for WorkerId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("{}", self.0))
    }
}

impl core::fmt::Debug for WorkerId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Display::fmt(&self, f)
    }
}

impl From<WorkerId> for String {
    fn from(val: WorkerId) -> Self {
        val.0
    }
}

impl From<String> for WorkerId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Holds the information needed to uniquely identify an action
/// and if it is cacheable or not.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionUniqueQualifier {
    /// The action is cacheable.
    #[serde(alias = "Cachable")] // Pre 0.7.0 spelling
    Cacheable(ActionUniqueKey),
    /// The action is uncacheable.
    #[serde(alias = "Uncachable")] // Pre 0.7.0 spelling
    Uncacheable(ActionUniqueKey),
}

impl MetricsComponent for ActionUniqueQualifier {
    fn publish(
        &self,
        _kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let (cacheable, action) = match self {
            Self::Cacheable(action) => (true, action),
            Self::Uncacheable(action) => (false, action),
        };
        publish!(
            cacheable,
            &cacheable,
            MetricKind::Default,
            "If the action is cacheable.",
            ""
        );
        action.publish(MetricKind::Component, field_metadata)?;
        Ok(MetricPublishKnownKindData::Component)
    }
}

impl ActionUniqueQualifier {
    /// Get the `instance_name` of the action.
    pub const fn instance_name(&self) -> &String {
        match self {
            Self::Cacheable(action) | Self::Uncacheable(action) => &action.instance_name,
        }
    }

    /// Get the digest function of the action.
    pub const fn digest_function(&self) -> DigestHasherFunc {
        match self {
            Self::Cacheable(action) | Self::Uncacheable(action) => action.digest_function,
        }
    }

    /// Get the digest of the action.
    pub const fn digest(&self) -> DigestInfo {
        match self {
            Self::Cacheable(action) | Self::Uncacheable(action) => action.digest,
        }
    }
}

impl Display for ActionUniqueQualifier {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (cacheable, unique_key) = match self {
            Self::Cacheable(action) => (true, action),
            Self::Uncacheable(action) => (false, action),
        };
        f.write_fmt(format_args!(
            // Note: We use underscores because it makes escaping easier
            // for redis.
            "{}_{}_{}_{}_{}",
            unique_key.instance_name,
            unique_key.digest_function,
            unique_key.digest.packed_hash(),
            unique_key.digest.size_bytes(),
            if cacheable { 'c' } else { 'u' },
        ))
    }
}

/// This is a utility struct used to make it easier to match `ActionInfos` in a
/// `HashMap` without needing to construct an entire `ActionInfo`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, MetricsComponent)]
pub struct ActionUniqueKey {
    /// Name of instance group this action belongs to.
    #[metric(help = "Name of instance group this action belongs to.")]
    pub instance_name: String,
    /// The digest function this action expects.
    #[metric(help = "The digest function this action expects.")]
    pub digest_function: DigestHasherFunc,
    /// Digest of the underlying `Action`.
    #[metric(help = "Digest of the underlying Action.")]
    pub digest: DigestInfo,
}

impl Display for ActionUniqueKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!(
            "{}/{}/{}",
            self.instance_name, self.digest_function, self.digest,
        ))
    }
}

/// Information needed to execute an action. This struct is used over bazel's proto `Action`
/// for simplicity and offers a `salt`, which is useful to ensure during hashing (for dicts)
/// to ensure we never match against another `ActionInfo` (when a task should never be cached).
/// This struct must be 100% compatible with `ExecuteRequest` struct in `remote_execution.proto`
/// except for the salt field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, MetricsComponent)]
pub struct ActionInfo {
    /// Digest of the underlying `Command`.
    #[metric(help = "Digest of the underlying Command.")]
    pub command_digest: DigestInfo,
    /// Digest of the underlying `Directory`.
    #[metric(help = "Digest of the underlying Directory.")]
    pub input_root_digest: DigestInfo,
    /// Timeout of the action.
    #[metric(help = "Timeout of the action.")]
    pub timeout: Duration,
    /// The properties rules that must be applied when finding a worker that can run this action.
    #[metric(group = "platform_properties")]
    pub platform_properties: HashMap<String, String>,
    /// The priority of the action. Higher value means it should execute faster.
    #[metric(help = "The priority of the action. Higher value means it should execute faster.")]
    pub priority: i32,
    /// When this action started to be loaded from the CAS.
    #[metric(help = "When this action started to be loaded from the CAS.")]
    pub load_timestamp: SystemTime,
    /// When this action was created.
    #[metric(help = "When this action was created.")]
    pub insert_timestamp: SystemTime,
    /// Info used to uniquely identify this `ActionInfo` and if it is cacheable.
    /// This is primarily used to join actions/operations together using this key.
    #[metric(help = "Info used to uniquely identify this ActionInfo and if it is cacheable.")]
    pub unique_qualifier: ActionUniqueQualifier,
}

impl ActionInfo {
    #[inline]
    pub const fn instance_name(&self) -> &String {
        self.unique_qualifier.instance_name()
    }

    /// Returns the underlying digest of the `Action`.
    #[inline]
    pub const fn digest(&self) -> DigestInfo {
        self.unique_qualifier.digest()
    }

    pub fn try_from_action_and_execute_request(
        execute_request: ExecuteRequest,
        action: Action,
        load_timestamp: SystemTime,
        queued_timestamp: SystemTime,
    ) -> Result<Self, Error> {
        let unique_key = ActionUniqueKey {
            instance_name: execute_request.instance_name,
            digest_function: DigestHasherFunc::try_from(execute_request.digest_function)
                .err_tip(|| format!("Could not find digest_function in try_from_action_and_execute_request {:?}", execute_request.digest_function))?,
            digest: execute_request
                .action_digest
                .err_tip(|| "Expected action_digest to exist on ExecuteRequest")?
                .try_into()?,
        };
        let unique_qualifier = if execute_request.skip_cache_lookup {
            ActionUniqueQualifier::Uncacheable(unique_key)
        } else {
            ActionUniqueQualifier::Cacheable(unique_key)
        };

        let proto_properties = action.platform.unwrap_or_default();
        let mut platform_properties = HashMap::with_capacity(proto_properties.properties.len());
        for property in proto_properties.properties {
            platform_properties.insert(property.name, property.value);
        }

        Ok(Self {
            command_digest: action
                .command_digest
                .err_tip(|| "Expected command_digest to exist on Action")?
                .try_into()?,
            input_root_digest: action
                .input_root_digest
                .err_tip(|| "Expected input_root_digest to exist on Action")?
                .try_into()?,
            timeout: action
                .timeout
                .unwrap_or_default()
                .try_into()
                .map_err(|_| make_input_err!("Failed convert proto duration to system duration"))?,
            platform_properties,
            priority: execute_request
                .execution_policy
                .unwrap_or_default()
                .priority,
            load_timestamp,
            insert_timestamp: queued_timestamp,
            unique_qualifier,
        })
    }
}

impl From<&ActionInfo> for ExecuteRequest {
    fn from(val: &ActionInfo) -> Self {
        let digest = val.digest().into();
        let (skip_cache_lookup, unique_qualifier) = match &val.unique_qualifier {
            ActionUniqueQualifier::Cacheable(unique_qualifier) => (false, unique_qualifier),
            ActionUniqueQualifier::Uncacheable(unique_qualifier) => (true, unique_qualifier),
        };
        Self {
            instance_name: unique_qualifier.instance_name.clone(),
            action_digest: Some(digest),
            skip_cache_lookup,
            execution_policy: None,     // Not used in the worker.
            results_cache_policy: None, // Not used in the worker.
            digest_function: unique_qualifier.digest_function.proto_digest_func().into(),
        }
    }
}

/// Simple utility struct to determine if a string is representing a full path or
/// just the name of the file.
/// This is in order to be able to reuse the same struct instead of building different
/// structs when converting `FileInfo` -> {`OutputFile`, `FileNode`} and other similar
/// structs.
#[derive(Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub enum NameOrPath {
    Name(String),
    Path(String),
}

impl PartialOrd for NameOrPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NameOrPath {
    fn cmp(&self, other: &Self) -> Ordering {
        let self_lexical_name = match self {
            Self::Name(name) => name,
            Self::Path(path) => path,
        };
        let other_lexical_name = match other {
            Self::Name(name) => name,
            Self::Path(path) => path,
        };
        self_lexical_name.cmp(other_lexical_name)
    }
}

/// Represents an individual file and associated metadata.
/// This struct must be 100% compatible with `OutputFile` and `FileNode` structs
/// in `remote_execution.proto`.
#[derive(Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub name_or_path: NameOrPath,
    pub digest: DigestInfo,
    pub is_executable: bool,
}

impl TryFrom<FileInfo> for FileNode {
    type Error = Error;

    fn try_from(val: FileInfo) -> Result<Self, Error> {
        match val.name_or_path {
            NameOrPath::Path(_) => Err(make_input_err!(
                "Cannot return a FileInfo that uses a NameOrPath::Path(), it must be a NameOrPath::Name()"
            )),
            NameOrPath::Name(name) => Ok(Self {
                name,
                digest: Some((&val.digest).into()),
                is_executable: val.is_executable,
                node_properties: None, // Not supported.
            }),
        }
    }
}

impl TryFrom<OutputFile> for FileInfo {
    type Error = Error;

    fn try_from(output_file: OutputFile) -> Result<Self, Error> {
        Ok(Self {
            name_or_path: NameOrPath::Path(output_file.path),
            digest: output_file
                .digest
                .err_tip(|| "Expected digest to exist on OutputFile")?
                .try_into()?,
            is_executable: output_file.is_executable,
        })
    }
}

impl TryFrom<FileInfo> for OutputFile {
    type Error = Error;

    fn try_from(val: FileInfo) -> Result<Self, Error> {
        match val.name_or_path {
            NameOrPath::Name(_) => Err(make_input_err!(
                "Cannot return a FileInfo that uses a NameOrPath::Name(), it must be a NameOrPath::Path()"
            )),
            NameOrPath::Path(path) => Ok(Self {
                path,
                digest: Some((&val.digest).into()),
                is_executable: val.is_executable,
                contents: Bytes::default(),
                node_properties: None, // Not supported.
            }),
        }
    }
}

/// Represents an individual symlink file and associated metadata.
/// This struct must be 100% compatible with `SymlinkNode` and `OutputSymlink`.
#[derive(Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct SymlinkInfo {
    pub name_or_path: NameOrPath,
    pub target: String,
}

impl TryFrom<SymlinkNode> for SymlinkInfo {
    type Error = Error;

    fn try_from(symlink_node: SymlinkNode) -> Result<Self, Error> {
        Ok(Self {
            name_or_path: NameOrPath::Name(symlink_node.name),
            target: symlink_node.target,
        })
    }
}

impl TryFrom<SymlinkInfo> for SymlinkNode {
    type Error = Error;

    fn try_from(val: SymlinkInfo) -> Result<Self, Error> {
        match val.name_or_path {
            NameOrPath::Path(_) => Err(make_input_err!(
                "Cannot return a SymlinkInfo that uses a NameOrPath::Path(), it must be a NameOrPath::Name()"
            )),
            NameOrPath::Name(name) => Ok(Self {
                name,
                target: val.target,
                node_properties: None, // Not supported.
            }),
        }
    }
}

impl TryFrom<OutputSymlink> for SymlinkInfo {
    type Error = Error;

    fn try_from(output_symlink: OutputSymlink) -> Result<Self, Error> {
        Ok(Self {
            name_or_path: NameOrPath::Path(output_symlink.path),
            target: output_symlink.target,
        })
    }
}

impl TryFrom<SymlinkInfo> for OutputSymlink {
    type Error = Error;

    fn try_from(val: SymlinkInfo) -> Result<Self, Error> {
        match val.name_or_path {
            NameOrPath::Path(path) => {
                Ok(Self {
                    path,
                    target: val.target,
                    node_properties: None, // Not supported.
                })
            }
            NameOrPath::Name(_) => Err(make_input_err!(
                "Cannot return a SymlinkInfo that uses a NameOrPath::Name(), it must be a NameOrPath::Path()"
            )),
        }
    }
}

/// Represents an individual directory file and associated metadata.
/// This struct must be 100% compatible with `SymlinkNode` and `OutputSymlink`.
#[derive(Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryInfo {
    pub path: String,
    pub tree_digest: DigestInfo,
}

impl TryFrom<OutputDirectory> for DirectoryInfo {
    type Error = Error;

    fn try_from(output_directory: OutputDirectory) -> Result<Self, Error> {
        Ok(Self {
            path: output_directory.path,
            tree_digest: output_directory
                .tree_digest
                .err_tip(|| "Expected tree_digest to exist in OutputDirectory")?
                .try_into()?,
        })
    }
}

impl From<DirectoryInfo> for OutputDirectory {
    fn from(val: DirectoryInfo) -> Self {
        Self {
            path: val.path,
            tree_digest: Some(val.tree_digest.into()),
            is_topologically_sorted: false,
        }
    }
}

/// Represents the metadata associated with the execution result.
/// This struct must be 100% compatible with `ExecutedActionMetadata`.
#[derive(Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionMetadata {
    pub worker: String,
    pub queued_timestamp: SystemTime,
    pub worker_start_timestamp: SystemTime,
    pub worker_completed_timestamp: SystemTime,
    pub input_fetch_start_timestamp: SystemTime,
    pub input_fetch_completed_timestamp: SystemTime,
    pub execution_start_timestamp: SystemTime,
    pub execution_completed_timestamp: SystemTime,
    pub output_upload_start_timestamp: SystemTime,
    pub output_upload_completed_timestamp: SystemTime,
}

impl Default for ExecutionMetadata {
    fn default() -> Self {
        Self {
            worker: String::new(),
            queued_timestamp: SystemTime::UNIX_EPOCH,
            worker_start_timestamp: SystemTime::UNIX_EPOCH,
            worker_completed_timestamp: SystemTime::UNIX_EPOCH,
            input_fetch_start_timestamp: SystemTime::UNIX_EPOCH,
            input_fetch_completed_timestamp: SystemTime::UNIX_EPOCH,
            execution_start_timestamp: SystemTime::UNIX_EPOCH,
            execution_completed_timestamp: SystemTime::UNIX_EPOCH,
            output_upload_start_timestamp: SystemTime::UNIX_EPOCH,
            output_upload_completed_timestamp: SystemTime::UNIX_EPOCH,
        }
    }
}

impl From<ExecutionMetadata> for ExecutedActionMetadata {
    fn from(val: ExecutionMetadata) -> Self {
        Self {
            worker: val.worker,
            queued_timestamp: Some(val.queued_timestamp.into()),
            worker_start_timestamp: Some(val.worker_start_timestamp.into()),
            worker_completed_timestamp: Some(val.worker_completed_timestamp.into()),
            input_fetch_start_timestamp: Some(val.input_fetch_start_timestamp.into()),
            input_fetch_completed_timestamp: Some(val.input_fetch_completed_timestamp.into()),
            execution_start_timestamp: Some(val.execution_start_timestamp.into()),
            execution_completed_timestamp: Some(val.execution_completed_timestamp.into()),
            output_upload_start_timestamp: Some(val.output_upload_start_timestamp.into()),
            output_upload_completed_timestamp: Some(val.output_upload_completed_timestamp.into()),
            virtual_execution_duration: val
                .execution_completed_timestamp
                .duration_since(val.execution_start_timestamp)
                .ok()
                .and_then(|duration| prost_types::Duration::try_from(duration).ok()),
            auxiliary_metadata: Vec::default(),
        }
    }
}

impl TryFrom<ExecutedActionMetadata> for ExecutionMetadata {
    type Error = Error;

    fn try_from(eam: ExecutedActionMetadata) -> Result<Self, Error> {
        Ok(Self {
            worker: eam.worker,
            queued_timestamp: eam
                .queued_timestamp
                .err_tip(|| "Expected queued_timestamp to exist in ExecutedActionMetadata")?
                .try_into()?,
            worker_start_timestamp: eam
                .worker_start_timestamp
                .err_tip(|| "Expected worker_start_timestamp to exist in ExecutedActionMetadata")?
                .try_into()?,
            worker_completed_timestamp: eam
                .worker_completed_timestamp
                .err_tip(|| {
                    "Expected worker_completed_timestamp to exist in ExecutedActionMetadata"
                })?
                .try_into()?,
            input_fetch_start_timestamp: eam
                .input_fetch_start_timestamp
                .err_tip(|| {
                    "Expected input_fetch_start_timestamp to exist in ExecutedActionMetadata"
                })?
                .try_into()?,
            input_fetch_completed_timestamp: eam
                .input_fetch_completed_timestamp
                .err_tip(|| {
                    "Expected input_fetch_completed_timestamp to exist in ExecutedActionMetadata"
                })?
                .try_into()?,
            execution_start_timestamp: eam
                .execution_start_timestamp
                .err_tip(|| {
                    "Expected execution_start_timestamp to exist in ExecutedActionMetadata"
                })?
                .try_into()?,
            execution_completed_timestamp: eam
                .execution_completed_timestamp
                .err_tip(|| {
                    "Expected execution_completed_timestamp to exist in ExecutedActionMetadata"
                })?
                .try_into()?,
            output_upload_start_timestamp: eam
                .output_upload_start_timestamp
                .err_tip(|| {
                    "Expected output_upload_start_timestamp to exist in ExecutedActionMetadata"
                })?
                .try_into()?,
            output_upload_completed_timestamp: eam
                .output_upload_completed_timestamp
                .err_tip(|| {
                    "Expected output_upload_completed_timestamp to exist in ExecutedActionMetadata"
                })?
                .try_into()?,
        })
    }
}

/// Represents the results of an execution.
/// This struct must be 100% compatible with `ActionResult` in `remote_execution.proto`.
#[derive(Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub output_files: Vec<FileInfo>,
    pub output_folders: Vec<DirectoryInfo>,
    pub output_directory_symlinks: Vec<SymlinkInfo>,
    pub output_file_symlinks: Vec<SymlinkInfo>,
    pub exit_code: i32,
    pub stdout_digest: DigestInfo,
    pub stderr_digest: DigestInfo,
    pub execution_metadata: ExecutionMetadata,
    pub server_logs: HashMap<String, DigestInfo>,
    pub error: Option<Error>,
    pub message: String,
}

impl Default for ActionResult {
    fn default() -> Self {
        Self {
            output_files: Vec::default(),
            output_folders: Vec::default(),
            output_directory_symlinks: Vec::default(),
            output_file_symlinks: Vec::default(),
            exit_code: INTERNAL_ERROR_EXIT_CODE,
            stdout_digest: DigestInfo::new([0u8; 32], 0),
            stderr_digest: DigestInfo::new([0u8; 32], 0),
            execution_metadata: ExecutionMetadata {
                worker: String::new(),
                queued_timestamp: SystemTime::UNIX_EPOCH,
                worker_start_timestamp: SystemTime::UNIX_EPOCH,
                worker_completed_timestamp: SystemTime::UNIX_EPOCH,
                input_fetch_start_timestamp: SystemTime::UNIX_EPOCH,
                input_fetch_completed_timestamp: SystemTime::UNIX_EPOCH,
                execution_start_timestamp: SystemTime::UNIX_EPOCH,
                execution_completed_timestamp: SystemTime::UNIX_EPOCH,
                output_upload_start_timestamp: SystemTime::UNIX_EPOCH,
                output_upload_completed_timestamp: SystemTime::UNIX_EPOCH,
            },
            server_logs: HashMap::default(),
            error: None,
            message: String::new(),
        }
    }
}

/// The execution status/stage. This should match `ExecutionStage::Value` in `remote_execution.proto`.
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
#[allow(
    clippy::large_enum_variant,
    reason = "TODO box the two relevant variants in a breaking release. Unfulfilled on nightly"
)]
pub enum ActionStage {
    /// Stage is unknown.
    Unknown,
    /// Checking the cache to see if action exists.
    CacheCheck,
    /// Action has been accepted and waiting for worker to take it.
    Queued,
    // TODO(palfrey) We need a way to know if the job was sent to a worker, but hasn't begun
    // execution yet.
    /// Worker is executing the action.
    Executing,
    /// Worker completed the work with result.
    Completed(ActionResult),
    /// Result was found from cache, don't decode the proto just to re-encode it.
    #[serde(serialize_with = "serialize_proto_result", skip_deserializing)]
    // The serialization step decodes this to an ActionResult which is serializable.
    // Since it will always be serialized as an ActionResult, we do not need to support
    // deserialization on this type at all.
    // In theory, serializing this should never happen so performance shouldn't be affected.
    CompletedFromCache(ProtoActionResult),
}

fn serialize_proto_result<S>(v: &ProtoActionResult, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let s = ActionResult::try_from(v.clone()).map_err(S::Error::custom)?;
    s.serialize(serializer)
}

impl ActionStage {
    pub const fn has_action_result(&self) -> bool {
        match self {
            Self::Unknown | Self::CacheCheck | Self::Queued | Self::Executing => false,
            Self::Completed(_) | Self::CompletedFromCache(_) => true,
        }
    }

    /// Returns true if the worker considers the action done and no longer needs to be tracked.
    // Note: This function is separate from `has_action_result()` to not mix the concept of
    //       "finished" with "has a result".
    pub const fn is_finished(&self) -> bool {
        self.has_action_result()
    }

    /// Returns if the stage enum is the same as the other stage enum, but
    /// does not compare the values of the enum.
    pub const fn is_same_stage(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Unknown, Self::Unknown)
                | (Self::CacheCheck, Self::CacheCheck)
                | (Self::Queued, Self::Queued)
                | (Self::Executing, Self::Executing)
                | (Self::Completed(_), Self::Completed(_))
                | (Self::CompletedFromCache(_), Self::CompletedFromCache(_))
        )
    }

    pub fn name(&self) -> String {
        match self {
            Self::Unknown => "Unknown".to_string(),
            Self::CacheCheck => "CacheCheck".to_string(),
            Self::Queued => "Queued".to_string(),
            Self::Executing => "Executing".to_string(),
            Self::Completed(_) => "Completed".to_string(),
            Self::CompletedFromCache(_) => "CompletedFromCache".to_string(),
        }
    }
}

impl MetricsComponent for ActionStage {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::String(self.name()))
    }
}

impl From<&ActionStage> for execution_stage::Value {
    fn from(val: &ActionStage) -> Self {
        match val {
            ActionStage::Unknown => Self::Unknown,
            ActionStage::CacheCheck => Self::CacheCheck,
            ActionStage::Queued => Self::Queued,
            ActionStage::Executing => Self::Executing,
            ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) => Self::Completed,
        }
    }
}

pub fn to_execute_response(action_result: ActionResult) -> ExecuteResponse {
    fn logs_from(server_logs: HashMap<String, DigestInfo>) -> HashMap<String, LogFile> {
        let mut logs = HashMap::with_capacity(server_logs.len());
        for (k, v) in server_logs {
            logs.insert(
                k.clone(),
                LogFile {
                    digest: Some(v.into()),
                    human_readable: false,
                },
            );
        }
        logs
    }

    let status = Some(
        action_result
            .error
            .clone()
            .map_or_else(Status::default, Into::into),
    );
    let message = action_result.message.clone();
    ExecuteResponse {
        server_logs: logs_from(action_result.server_logs.clone()),
        result: action_result.try_into().ok(),
        cached_result: false,
        status,
        message,
    }
}

impl From<ActionStage> for ExecuteResponse {
    fn from(val: ActionStage) -> Self {
        match val {
            // We don't have an execute response if we don't have the results. It is defined
            // behavior to return an empty proto struct.
            ActionStage::Unknown
            | ActionStage::CacheCheck
            | ActionStage::Queued
            | ActionStage::Executing => Self::default(),
            ActionStage::Completed(action_result) => to_execute_response(action_result),
            // Handled separately as there are no server logs and the action
            // result is already in Proto format.
            ActionStage::CompletedFromCache(proto_action_result) => Self {
                server_logs: HashMap::new(),
                result: Some(proto_action_result),
                cached_result: true,
                status: Some(Status::default()),
                message: String::new(), // Will be populated later if applicable.
            },
        }
    }
}

impl TryFrom<ActionResult> for ProtoActionResult {
    type Error = Error;

    fn try_from(val: ActionResult) -> Result<Self, Error> {
        let mut output_symlinks = Vec::with_capacity(
            val.output_file_symlinks.len() + val.output_directory_symlinks.len(),
        );
        output_symlinks.extend_from_slice(val.output_file_symlinks.as_slice());
        output_symlinks.extend_from_slice(val.output_directory_symlinks.as_slice());

        Ok(Self {
            output_files: val
                .output_files
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            output_file_symlinks: val
                .output_file_symlinks
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            output_symlinks: output_symlinks
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            output_directories: val
                .output_folders
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            output_directory_symlinks: val
                .output_directory_symlinks
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            exit_code: val.exit_code,
            stdout_raw: Bytes::default(),
            stdout_digest: Some(val.stdout_digest.into()),
            stderr_raw: Bytes::default(),
            stderr_digest: Some(val.stderr_digest.into()),
            execution_metadata: Some(val.execution_metadata.into()),
        })
    }
}

impl TryFrom<ProtoActionResult> for ActionResult {
    type Error = Error;

    fn try_from(val: ProtoActionResult) -> Result<Self, Error> {
        let output_file_symlinks = val
            .output_file_symlinks
            .into_iter()
            .map(|output_symlink| {
                SymlinkInfo::try_from(output_symlink)
                    .err_tip(|| "Output File Symlinks could not be converted to SymlinkInfo")
            })
            .collect::<Result<Vec<_>, _>>()?;

        let output_directory_symlinks = val
            .output_directory_symlinks
            .into_iter()
            .map(|output_symlink| {
                SymlinkInfo::try_from(output_symlink)
                    .err_tip(|| "Output File Symlinks could not be converted to SymlinkInfo")
            })
            .collect::<Result<Vec<_>, _>>()?;

        let output_files = val
            .output_files
            .into_iter()
            .map(|output_file| {
                output_file
                    .try_into()
                    .err_tip(|| "Output File could not be converted")
            })
            .collect::<Result<Vec<_>, _>>()?;

        let output_folders = val
            .output_directories
            .into_iter()
            .map(|output_directory| {
                output_directory
                    .try_into()
                    .err_tip(|| "Output File could not be converted")
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            output_files,
            output_folders,
            output_file_symlinks,
            output_directory_symlinks,
            exit_code: val.exit_code,
            stdout_digest: val
                .stdout_digest
                .err_tip(|| "Expected stdout_digest to be set on ExecuteResponse msg")?
                .try_into()?,
            stderr_digest: val
                .stderr_digest
                .err_tip(|| "Expected stderr_digest to be set on ExecuteResponse msg")?
                .try_into()?,
            execution_metadata: val
                .execution_metadata
                .err_tip(|| "Expected execution_metadata to be set on ExecuteResponse msg")?
                .try_into()?,
            server_logs: HashMap::default(),
            error: None,
            message: String::new(),
        })
    }
}

impl TryFrom<ExecuteResponse> for ActionStage {
    type Error = Error;

    fn try_from(execute_response: ExecuteResponse) -> Result<Self, Error> {
        let proto_action_result = execute_response
            .result
            .err_tip(|| "Expected result to be set on ExecuteResponse msg")?;
        let action_result = ActionResult {
            output_files: proto_action_result
                .output_files
                .try_map(TryInto::try_into)?,
            output_directory_symlinks: proto_action_result
                .output_directory_symlinks
                .try_map(TryInto::try_into)?,
            output_file_symlinks: proto_action_result
                .output_file_symlinks
                .try_map(TryInto::try_into)?,
            output_folders: proto_action_result
                .output_directories
                .try_map(TryInto::try_into)?,
            exit_code: proto_action_result.exit_code,

            stdout_digest: proto_action_result
                .stdout_digest
                .err_tip(|| "Expected stdout_digest to be set on ExecuteResponse msg")?
                .try_into()?,
            stderr_digest: proto_action_result
                .stderr_digest
                .err_tip(|| "Expected stderr_digest to be set on ExecuteResponse msg")?
                .try_into()?,
            execution_metadata: proto_action_result
                .execution_metadata
                .err_tip(|| "Expected execution_metadata to be set on ExecuteResponse msg")?
                .try_into()?,
            server_logs: execute_response.server_logs.try_map(|v| {
                v.digest
                    .err_tip(|| "Expected digest to be set on LogFile msg")?
                    .try_into()
            })?,
            error: execute_response
                .status
                .clone()
                .and_then(|v| if v.code == 0 { None } else { Some(v.into()) }),
            message: execute_response.message,
        };

        if execute_response.cached_result {
            return Ok(Self::CompletedFromCache(action_result.try_into()?));
        }
        Ok(Self::Completed(action_result))
    }
}

// TODO: Should be able to remove this after tokio-rs/prost#299
trait TypeUrl: Message {
    const TYPE_URL: &'static str;
}

impl TypeUrl for ExecuteResponse {
    const TYPE_URL: &'static str =
        "type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse";
}

impl TypeUrl for ExecuteOperationMetadata {
    const TYPE_URL: &'static str =
        "type.googleapis.com/build.bazel.remote.execution.v2.ExecuteOperationMetadata";
}

fn from_any<T>(message: &Any) -> Result<T, Error>
where
    T: TypeUrl + Default,
{
    error_if!(
        message.type_url != T::TYPE_URL,
        "Incorrect type when decoding Any. {} != {}",
        message.type_url,
        T::TYPE_URL.to_string()
    );
    Ok(T::decode(message.value.as_slice())?)
}

fn to_any<T>(message: &T) -> Any
where
    T: TypeUrl,
{
    Any {
        type_url: T::TYPE_URL.to_string(),
        value: message.encode_to_vec(),
    }
}

/// Current state of the action.
/// This must be 100% compatible with `Operation` in `google/longrunning/operations.proto`.
#[derive(Debug, Clone, Serialize, Deserialize, MetricsComponent)]
pub struct ActionState {
    #[metric(help = "The current stage of the action.")]
    pub stage: ActionStage,
    #[metric(help = "Last time this action changed stage")]
    pub last_transition_timestamp: SystemTime,
    #[metric(help = "The unique identifier of the action.")]
    pub client_operation_id: OperationId,
    #[metric(help = "The digest of the action.")]
    pub action_digest: DigestInfo,
}

impl Display for ActionState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "stage={} last_transition={} client_operation_id={} action_digest={}",
            self.stage.name(),
            self.last_transition_timestamp.elapsed().map_or_else(
                |_| "<unknown duration>".to_string(),
                |d| { format_duration(d).to_string() }
            ),
            self.client_operation_id,
            self.action_digest
        )
    }
}

impl PartialOrd for ActionState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ActionState {
    fn cmp(&self, other: &Self) -> Ordering {
        self.last_transition_timestamp
            .cmp(&other.last_transition_timestamp)
    }
}

impl PartialEq for ActionState {
    fn eq(&self, other: &Self) -> bool {
        // Ignore last_transition_timestamp as the actions can still be the same even if they happened at different times
        self.stage == other.stage
            && self.client_operation_id == other.client_operation_id
            && self.action_digest == other.action_digest
    }
}

impl Eq for ActionState {}

impl ActionState {
    pub fn try_from_operation(
        operation: Operation,
        client_operation_id: OperationId,
    ) -> Result<Self, Error> {
        let metadata = from_any::<ExecuteOperationMetadata>(
            &operation
                .metadata
                .err_tip(|| "No metadata in upstream operation")?,
        )
        .err_tip(|| "Could not decode metadata in upstream operation")?;

        let stage = match execution_stage::Value::try_from(metadata.stage).err_tip(|| {
            format!(
                "Could not convert {} to execution_stage::Value",
                metadata.stage
            )
        })? {
            execution_stage::Value::Unknown => ActionStage::Unknown,
            execution_stage::Value::CacheCheck => ActionStage::CacheCheck,
            execution_stage::Value::Queued => ActionStage::Queued,
            execution_stage::Value::Executing => ActionStage::Executing,
            execution_stage::Value::Completed => {
                let execute_response = operation
                    .result
                    .err_tip(|| "No result data for completed upstream action")?;
                match execute_response {
                    LongRunningResult::Error(error) => ActionStage::Completed(ActionResult {
                        error: Some(error.into()),
                        ..ActionResult::default()
                    }),
                    LongRunningResult::Response(response) => {
                        // Could be Completed, CompletedFromCache or Error.
                        from_any::<ExecuteResponse>(&response)
                            .err_tip(|| {
                                "Could not decode result structure for completed upstream action"
                            })?
                            .try_into()?
                    }
                }
            }
        };

        let action_digest = metadata
            .action_digest
            .err_tip(|| "No action_digest in upstream operation")?
            .try_into()
            .err_tip(|| "Could not convert action_digest into DigestInfo")?;

        Ok(Self {
            stage,
            client_operation_id,
            action_digest,
            last_transition_timestamp: SystemTime::now(),
        })
    }

    pub fn as_operation(&self, client_operation_id: OperationId) -> Operation {
        let stage = Into::<execution_stage::Value>::into(&self.stage) as i32;
        let name = client_operation_id.into_string();

        let result = if self.stage.has_action_result() {
            let execute_response: ExecuteResponse = self.stage.clone().into();
            Some(LongRunningResult::Response(to_any(&execute_response)))
        } else {
            None
        };
        let digest = Some(self.action_digest.into());

        let metadata = ExecuteOperationMetadata {
            stage,
            action_digest: digest,
            // TODO(palfrey) We should support stderr/stdout streaming.
            stdout_stream_name: String::default(),
            stderr_stream_name: String::default(),
            partial_execution_metadata: None,
        };

        Operation {
            name,
            metadata: Some(to_any(&metadata)),
            done: result.is_some(),
            result,
        }
    }
}
