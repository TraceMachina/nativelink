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

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use prost::Message;
use prost_types::Any;
use sha2::{digest::Update as _, Digest as _, Sha256};

use common::{DigestInfo, HashMapExt, VecExt};
use error::{error_if, make_input_err, Error, ResultExt};
use platform_property_manager::PlatformProperties;
use prost::bytes::Bytes;
use proto::build::bazel::remote::execution::v2::{
    execution_stage, Action, ActionResult as ProtoActionResult, ExecuteOperationMetadata, ExecuteRequest,
    ExecuteResponse, ExecutedActionMetadata, FileNode, LogFile, OutputDirectory, OutputFile, OutputSymlink,
    SymlinkNode,
};
use proto::google::longrunning::{operation::Result as LongRunningResult, Operation};
use proto::google::rpc::Status;

/// Default priority remote execution jobs will get when not provided.
pub const DEFAULT_EXECUTION_PRIORITY: i32 = 0;

/// This is a utility struct used to make it easier to match `ActionInfos` in a
/// `HashMap` without needing to construct an entire `ActionInfo`.
/// Since the hashing only needs the digest and salt we can just alias them here
/// and point the original `ActionInfo` structs to reference these structs for
/// it's hashing functions.
#[derive(Debug, Clone)]
pub struct ActionInfoHashKey {
    /// Digest of the underlying `Action`.
    pub digest: DigestInfo,
    /// Salt that can be filled with a random number to ensure no `ActionInfo` will be a match
    /// to another `ActionInfo` in the scheduler. When caching is wanted this value is usually
    /// zero.
    pub salt: u64,
}

impl ActionInfoHashKey {
    /// Utility function used to make a unique hash of the digest including the salt.
    pub fn get_hash(&self) -> [u8; 32] {
        Sha256::new()
            .chain(self.digest.packed_hash)
            .chain(self.digest.size_bytes.to_le_bytes())
            .chain(self.salt.to_le_bytes())
            .finalize()
            .into()
    }
}

/// Information needed to execute an action. This struct is used over bazel's proto `Action`
/// for simplicity and offers a `salt`, which is useful to ensure during hashing (for dicts)
/// to ensure we never match against another `ActionInfo` (when a task should never be cached).
/// This struct must be 100% compatible with `ExecuteRequest` struct in `remote_execution.proto`
/// except for the salt field.
#[derive(Clone, Debug)]
pub struct ActionInfo {
    /// Instance name used to send the request.
    pub instance_name: String,
    /// Digest of the underlying `Command`.
    pub command_digest: DigestInfo,
    /// Digest of the underlying `Directory`.
    pub input_root_digest: DigestInfo,
    /// Timeout of the action.
    pub timeout: Duration,
    /// The properties rules that must be applied when finding a worker that can run this action.
    pub platform_properties: PlatformProperties,
    /// The priority of the action. Higher value means it should execute faster.
    pub priority: i32,
    /// When this action started to be loaded from the CAS.
    pub load_timestamp: SystemTime,
    /// When this action was created.
    pub insert_timestamp: SystemTime,

    /// Info used to uniquely identify this ActionInfo. Normally the hash function would just
    /// use the fields it needs and you wouldn't need to separate them, however we have a use
    /// case where we sometimes want to lookup an entry in a HashMap, but we don't have the
    /// info to construct an entire ActionInfo. In such case we construct only a ActionInfoHashKey
    /// then use that object to lookup the entry in the map. The root problem is that HashMap
    /// requires `ActionInfo :Borrow<ActionInfoHashKey>` in order for this to work, which means
    /// we need to be able to return a &ActionInfoHashKey from ActionInfo, but since we cannot
    /// return a temporary reference we must have an object tied to ActionInfo's lifetime and
    /// return it's reference.
    pub unique_qualifier: ActionInfoHashKey,

    /// Whether to try looking up this action in the cache.
    pub skip_cache_lookup: bool,
}

impl ActionInfo {
    /// Returns the underlying digest of the `Action`.
    #[inline]
    pub const fn digest(&self) -> &DigestInfo {
        &self.unique_qualifier.digest
    }

    /// Returns the salt used for cache busting/hashing.
    #[inline]
    pub const fn salt(&self) -> &u64 {
        &self.unique_qualifier.salt
    }

    pub fn try_from_action_and_execute_request_with_salt(
        execute_request: ExecuteRequest,
        action: Action,
        salt: u64,
        load_timestamp: SystemTime,
        queued_timestamp: SystemTime,
    ) -> Result<Self, Error> {
        Ok(Self {
            instance_name: execute_request.instance_name,
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
            platform_properties: action.platform.unwrap_or_default().try_into()?,
            priority: execute_request.execution_policy.unwrap_or_default().priority,
            load_timestamp,
            insert_timestamp: queued_timestamp,
            unique_qualifier: ActionInfoHashKey {
                digest: execute_request
                    .action_digest
                    .err_tip(|| "Expected action_digest to exist on ExecuteRequest")?
                    .try_into()?,
                salt,
            },
            skip_cache_lookup: execute_request.skip_cache_lookup,
        })
    }
}

impl From<ActionInfo> for ExecuteRequest {
    fn from(val: ActionInfo) -> Self {
        let digest = val.digest().into();
        Self {
            instance_name: val.instance_name,
            action_digest: Some(digest),
            skip_cache_lookup: true,    // The worker should never cache lookup.
            execution_policy: None,     // Not used in the worker.
            results_cache_policy: None, // Not used in the worker.
        }
    }
}

// Note: Hashing, Eq, and Ord matching on this struct is unique. Normally these functions
// must play well with each other, but in our case the following rules apply:
// * Hash - Hashing must be unique on the exact command being run and must never match
//          when do_not_cache is enabled, but must be consistent between identical data
//          hashes.
// * Eq   - Same as hash.
// * Ord  - Used when sorting `ActionInfo` together. The only major sorting is priority and
//          insert_timestamp, everything else is undefined, but must be deterministic.
impl Hash for ActionInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        ActionInfoHashKey::hash(&self.unique_qualifier, state);
    }
}

impl PartialEq for ActionInfo {
    fn eq(&self, other: &Self) -> bool {
        ActionInfoHashKey::eq(&self.unique_qualifier, &other.unique_qualifier)
    }
}

impl Eq for ActionInfo {}

impl Ord for ActionInfo {
    fn cmp(&self, other: &Self) -> Ordering {
        // Want the highest priority on top, but the lowest insert_timestamp.
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.insert_timestamp.cmp(&self.insert_timestamp))
            .then_with(|| self.salt().cmp(other.salt()))
            .then_with(|| self.digest().size_bytes.cmp(&other.digest().size_bytes))
            .then_with(|| self.digest().packed_hash.cmp(&other.digest().packed_hash))
    }
}

impl PartialOrd for ActionInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let cmp = self
            .priority
            .cmp(&other.priority)
            .then_with(|| other.insert_timestamp.cmp(&self.insert_timestamp))
            .then_with(|| self.salt().cmp(other.salt()));
        if cmp == Ordering::Equal {
            return None;
        }
        Some(cmp)
    }
}

impl Borrow<ActionInfoHashKey> for Arc<ActionInfo> {
    #[inline]
    fn borrow(&self) -> &ActionInfoHashKey {
        &self.unique_qualifier
    }
}

impl Hash for ActionInfoHashKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Digest is unique, so hashing it is all we need.
        self.digest.hash(state);
        self.salt.hash(state);
    }
}

impl PartialEq for ActionInfoHashKey {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest && self.salt == other.salt
    }
}

impl Eq for ActionInfoHashKey {}

/// Simple utility struct to determine if a string is representing a full path or
/// just the name of the file.
/// This is in order to be able to reuse the same struct instead of building different
/// structs when converting `FileInfo` -> {`OutputFile`, `FileNode`} and other similar
/// structs.
#[derive(Eq, PartialEq, Debug, Clone)]
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
#[derive(Eq, PartialEq, Debug, Clone)]
pub struct FileInfo {
    pub name_or_path: NameOrPath,
    pub digest: DigestInfo,
    pub is_executable: bool,
}

//TODO: Make this TryFrom.
impl From<FileInfo> for FileNode {
    fn from(val: FileInfo) -> Self {
        let NameOrPath::Name(name) = val.name_or_path else {
            panic!("Cannot return a FileInfo that uses a NameOrPath::Path(), it must be a NameOrPath::Name()");
        };
        Self {
            name,
            digest: Some((&val.digest).into()),
            is_executable: val.is_executable,
            node_properties: Option::default(), // Not supported.
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

//TODO: Make this TryFrom.
impl From<FileInfo> for OutputFile {
    fn from(val: FileInfo) -> Self {
        let NameOrPath::Path(path) = val.name_or_path else {
            panic!("Cannot return a FileInfo that uses a NameOrPath::Name(), it must be a NameOrPath::Path()");
        };
        Self {
            path,
            digest: Some((&val.digest).into()),
            is_executable: val.is_executable,
            contents: Bytes::default(),
            node_properties: Option::default(), // Not supported.
        }
    }
}

/// Represents an individual symlink file and associated metadata.
/// This struct must be 100% compatible with `SymlinkNode` and `OutputSymlink`.
#[derive(Eq, PartialEq, Debug, Clone)]
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

// TODO: Make this TryFrom.
impl From<SymlinkInfo> for SymlinkNode {
    fn from(val: SymlinkInfo) -> Self {
        let NameOrPath::Name(name) = val.name_or_path else {
            panic!("Cannot return a SymlinkInfo that uses a NameOrPath::Path(), it must be a NameOrPath::Name()");
        };
        Self {
            name,
            target: val.target,
            node_properties: Option::default(), // Not supported.
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

// TODO: Make this TryFrom.
impl From<SymlinkInfo> for OutputSymlink {
    fn from(val: SymlinkInfo) -> Self {
        let NameOrPath::Path(path) = val.name_or_path else {
            panic!("Cannot return a SymlinkInfo that uses a NameOrPath::Path(), it must be a NameOrPath::Name()");
        };
        Self {
            path,
            target: val.target,
            node_properties: Option::default(), // Not supported.
        }
    }
}

/// Represents an individual directory file and associated metadata.
/// This struct must be 100% compatible with `SymlinkNode` and `OutputSymlink`.
#[derive(Eq, PartialEq, Debug, Clone)]
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
        }
    }
}

/// Represents the metadata associated with the execution result.
/// This struct must be 100% compatible with `ExecutedActionMetadata`.
#[derive(Eq, PartialEq, Debug, Clone)]
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
                .err_tip(|| "Expected worker_completed_timestamp to exist in ExecutedActionMetadata")?
                .try_into()?,
            input_fetch_start_timestamp: eam
                .input_fetch_start_timestamp
                .err_tip(|| "Expected input_fetch_start_timestamp to exist in ExecutedActionMetadata")?
                .try_into()?,
            input_fetch_completed_timestamp: eam
                .input_fetch_completed_timestamp
                .err_tip(|| "Expected input_fetch_completed_timestamp to exist in ExecutedActionMetadata")?
                .try_into()?,
            execution_start_timestamp: eam
                .execution_start_timestamp
                .err_tip(|| "Expected execution_start_timestamp to exist in ExecutedActionMetadata")?
                .try_into()?,
            execution_completed_timestamp: eam
                .execution_completed_timestamp
                .err_tip(|| "Expected execution_completed_timestamp to exist in ExecutedActionMetadata")?
                .try_into()?,
            output_upload_start_timestamp: eam
                .output_upload_start_timestamp
                .err_tip(|| "Expected output_upload_start_timestamp to exist in ExecutedActionMetadata")?
                .try_into()?,
            output_upload_completed_timestamp: eam
                .output_upload_completed_timestamp
                .err_tip(|| "Expected output_upload_completed_timestamp to exist in ExecutedActionMetadata")?
                .try_into()?,
        })
    }
}

/// Exit code sent if there is an internal error.
pub const INTERNAL_ERROR_EXIT_CODE: i32 = -178;

/// Represents the results of an execution.
/// This struct must be 100% compatible with `ActionResult` in `remote_execution.proto`.
#[derive(Eq, PartialEq, Debug, Clone)]
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
}

impl Default for ActionResult {
    fn default() -> Self {
        ActionResult {
            output_files: Default::default(),
            output_folders: Default::default(),
            output_directory_symlinks: Default::default(),
            output_file_symlinks: Default::default(),
            exit_code: INTERNAL_ERROR_EXIT_CODE,
            stdout_digest: DigestInfo::empty_digest(),
            stderr_digest: DigestInfo::empty_digest(),
            execution_metadata: ExecutionMetadata {
                worker: "".to_string(),
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
            server_logs: Default::default(),
        }
    }
}

/// The execution status/stage. This should match `ExecutionStage::Value` in `remote_execution.proto`.
#[derive(PartialEq, Debug, Clone)]
pub enum ActionStage {
    /// Stage is unknown.
    Unknown,
    /// Checking the cache to see if action exists.
    CacheCheck,
    /// Action has been accepted and waiting for worker to take it.
    Queued,
    // TODO(allada) We need a way to know if the job was sent to a worker, but hasn't begun
    // execution yet.
    /// Worker is executing the action.
    Executing,
    /// Worker completed the work with result.
    Completed(ActionResult),
    /// Result was found from cache, don't decode the proto just to re-encode it.
    CompletedFromCache(ProtoActionResult),
    /// Error or action failed with an exit code on the worker.
    /// This means that the job might have finished executing, but returned a non-zero
    /// exit code (for example a test failing or file not compilable). The `ActionResult`
    /// may contain better results than the `Error` message.
    Error((Error, ActionResult)),
}

impl ActionStage {
    pub const fn has_action_result(&self) -> bool {
        match self {
            Self::Unknown | Self::CacheCheck | Self::Queued | Self::Executing => false,
            Self::Completed(_) | Self::CompletedFromCache(_) | Self::Error(_) => true,
        }
    }
}

impl From<&ActionStage> for execution_stage::Value {
    fn from(val: &ActionStage) -> Self {
        match val {
            ActionStage::Unknown => Self::Unknown,
            ActionStage::CacheCheck => Self::CacheCheck,
            ActionStage::Queued => Self::Queued,
            ActionStage::Executing => Self::Executing,
            ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) | ActionStage::Error(_) => Self::Completed,
        }
    }
}

impl From<ActionStage> for ExecuteResponse {
    fn from(val: ActionStage) -> Self {
        const RESPONSE_MESSAGE: &str = "TODO(blaise.bruer) We should put a reference something like bb_browser";

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

        match val {
            // We don't have an execute response if we don't have the results. It is defined
            // behavior to return an empty proto struct.
            ActionStage::Unknown | ActionStage::CacheCheck | ActionStage::Queued | ActionStage::Executing => {
                Self::default()
            }

            ActionStage::Completed(action_result) => Self {
                server_logs: logs_from(action_result.server_logs.clone()),
                result: Some(action_result.into()),
                cached_result: false,
                status: Some(Status::default()),
                message: RESPONSE_MESSAGE.to_string(),
            },
            ActionStage::Error((error, action_result)) => Self {
                server_logs: logs_from(action_result.server_logs.clone()),
                result: Some(action_result.into()),
                cached_result: false,
                status: Some(error.into()),
                message: RESPONSE_MESSAGE.to_string(),
            },
            // Handled separately as there are no server logs and the action
            // result is already in Proto format.
            ActionStage::CompletedFromCache(proto_action_result) => Self {
                server_logs: HashMap::new(),
                result: Some(proto_action_result),
                cached_result: true,
                status: Some(Status::default()),
                message: RESPONSE_MESSAGE.to_string(),
            },
        }
    }
}

impl From<ActionResult> for ProtoActionResult {
    fn from(val: ActionResult) -> Self {
        let mut output_symlinks =
            Vec::with_capacity(val.output_file_symlinks.len() + val.output_directory_symlinks.len());
        output_symlinks.extend_from_slice(val.output_file_symlinks.as_slice());
        output_symlinks.extend_from_slice(val.output_directory_symlinks.as_slice());

        Self {
            output_files: val.output_files.into_iter().map(Into::into).collect(),
            output_file_symlinks: val.output_file_symlinks.into_iter().map(Into::into).collect(),
            output_symlinks: output_symlinks.into_iter().map(Into::into).collect(),
            output_directories: val.output_folders.into_iter().map(Into::into).collect(),
            output_directory_symlinks: val.output_directory_symlinks.into_iter().map(Into::into).collect(),
            exit_code: val.exit_code,
            stdout_raw: Bytes::default(),
            stdout_digest: Some(val.stdout_digest.into()),
            stderr_raw: Bytes::default(),
            stderr_digest: Some(val.stderr_digest.into()),
            execution_metadata: Some(val.execution_metadata.into()),
        }
    }
}

impl TryFrom<ExecuteResponse> for ActionStage {
    type Error = Error;

    fn try_from(execute_response: ExecuteResponse) -> Result<Self, Error> {
        let proto_action_result = execute_response
            .result
            .err_tip(|| "Expected result to be set on ExecuteResponse msg")?;
        let action_result = ActionResult {
            output_files: proto_action_result.output_files.try_map(TryInto::try_into)?,
            output_directory_symlinks: proto_action_result
                .output_directory_symlinks
                .try_map(TryInto::try_into)?,
            output_file_symlinks: proto_action_result.output_file_symlinks.try_map(TryInto::try_into)?,
            output_folders: proto_action_result.output_directories.try_map(TryInto::try_into)?,
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
        };

        let status = execute_response
            .status
            .err_tip(|| "Expected status to be set on ExecuteResponse")?;
        if status.code != tonic::Code::Ok as i32 {
            return Ok(Self::Error((status.into(), action_result)));
        }

        if execute_response.cached_result {
            return Ok(Self::CompletedFromCache(action_result.into()));
        }
        Ok(Self::Completed(action_result))
    }
}

impl TryFrom<Operation> for ActionState {
    type Error = Error;

    fn try_from(operation: Operation) -> Result<ActionState, Error> {
        let metadata_data = operation.metadata.err_tip(|| "No metadata in upstream operation")?;
        error_if!(
            metadata_data.type_url != "type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse",
            "Incorrect metadata structure in upstream operation. {} != type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse",
            metadata_data.type_url
        );
        let metadata = ExecuteOperationMetadata::decode(metadata_data.value.as_slice())
            .err_tip(|| "Could not decode metadata in upstream operation")?;

        let action_digest = metadata
            .action_digest
            .err_tip(|| "No action digest in upstream operation metadata")?
            .try_into()
            .err_tip(|| "Could not convert Digest to DigestInfo")?;

        let stage = match execution_stage::Value::from_i32(metadata.stage)
            .err_tip(|| format!("Could not convert {} to execution_stage::Value", metadata.stage))?
        {
            execution_stage::Value::Unknown => ActionStage::Unknown,
            execution_stage::Value::CacheCheck => ActionStage::CacheCheck,
            execution_stage::Value::Queued => ActionStage::Queued,
            execution_stage::Value::Executing => ActionStage::Executing,
            execution_stage::Value::Completed => {
                let execute_response = operation
                    .result
                    .err_tip(|| "No result data for completed upstream action")?;
                match execute_response {
                    LongRunningResult::Error(error) => ActionStage::Error((error.into(), ActionResult::default())),
                    LongRunningResult::Response(response) => {
                        // Could be Completed, CompletedFromCache or Error.
                        error_if!(
                            response.type_url != "type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse",
                            "Incorrect result structure for completed upstream action. {} != type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse",
                            response.type_url
                        );
                        ExecuteResponse::decode(response.value.as_slice())?.try_into()?
                    }
                }
            }
        };

        Ok(Self {
            name: operation.name,
            action_digest,
            stage,
        })
    }
}

/// Current state of the action.
/// This must be 100% compatible with `Operation` in `google/longrunning/operations.proto`.
#[derive(PartialEq, Debug, Clone)]
pub struct ActionState {
    pub name: String,
    pub action_digest: DigestInfo,
    pub stage: ActionStage,
}

impl From<ActionState> for Operation {
    fn from(val: ActionState) -> Self {
        let has_action_result = val.stage.has_action_result();
        let stage = Into::<execution_stage::Value>::into(&val.stage) as i32;
        let execute_response: ExecuteResponse = val.stage.into();

        let serialized_response = if has_action_result {
            execute_response.encode_to_vec()
        } else {
            vec![]
        };

        let metadata = ExecuteOperationMetadata {
            stage,
            action_digest: Some((&val.action_digest).into()),
            // TODO(blaise.bruer) We should support stderr/stdout streaming.
            stdout_stream_name: String::default(),
            stderr_stream_name: String::default(),
        };

        Self {
            name: val.name,
            metadata: Some(Any {
                type_url: "type.googleapis.com/build.bazel.remote.execution.v2.ExecuteOperationMetadata".to_string(),
                value: metadata.encode_to_vec(),
            }),
            done: has_action_result,
            result: Some(LongRunningResult::Response(Any {
                type_url: "type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse".to_string(),
                value: serialized_response,
            })),
        }
    }
}
