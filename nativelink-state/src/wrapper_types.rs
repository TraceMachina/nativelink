// Copyright 2023 The NativeLink Authors. All rights reserved.
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
use std::fmt;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use blake3::Hasher as Blake3Hasher;
use hex::FromHex;

use nativelink_error::{error_if, make_input_err, Error, ResultExt};
use nativelink_proto::build::bazel::remote::execution::v2::digest_function::Value as ProtoDigestFunction;
use nativelink_proto::build::bazel::remote::execution::v2::{
    execution_stage, Action, ActionResult as ProtoActionResult, Digest, ExecuteOperationMetadata, ExecuteRequest, ExecuteResponse, ExecutedActionMetadata, FileNode, LogFile, OutputDirectory, OutputFile, OutputSymlink, SymlinkNode
};
use nativelink_proto::google::longrunning::operation::Result as LongRunningResult;
use nativelink_proto::google::longrunning::Operation;
use nativelink_proto::google::rpc::Status;
use prost::bytes::Bytes;
use prost::Message;
use prost_types::Any;
use serde::{Deserialize, Serialize};
use serde::ser::{SerializeMap, Serializer};
use uuid::Uuid;


use nativelink_util::common::{HashMapExt, VecExt};
use nativelink_util::digest_hasher::{default_digest_hasher_func, DigestHasherFunc};
use nativelink_util::metrics_utils::{CollectorState, MetricsComponent};
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};

/// Default priority remote execution jobs will get when not provided.
pub const DEFAULT_EXECUTION_PRIORITY: i32 = 0;

pub type WorkerTimestamp = u64;

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RedisOperationId {
    pub unique_qualifier: RedisActionInfoHashKey,
    pub id: Uuid,
}

// TODO: Eventually we should make this it's own hash rather than delegate to RedisActionInfoHashKey.
impl Hash for RedisOperationId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        RedisActionInfoHashKey::hash(&self.unique_qualifier, state)
    }
}

impl RedisOperationId {
    pub fn new(unique_qualifier: RedisActionInfoHashKey) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            unique_qualifier
        }
    }

    /// Utility function used to make a unique hash of the digest including the salt.
    pub fn get_hash(&self) -> [u8; 32] {
        self.unique_qualifier.get_hash()
    }

    /// Returns the salt used for cache busting/hashing.
    #[inline]
    pub fn action_name(&self) -> String {
        self.unique_qualifier.action_name()
    }
}

impl TryFrom<&str> for RedisOperationId {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Error> {
        let (unique_qualifier, id) = value
            .split_once(':')
            .err_tip(|| "Invalid Id string - {value}")?;
        Ok(Self {
            unique_qualifier: RedisActionInfoHashKey::try_from(unique_qualifier)?,
            id: Uuid::parse_str(id).map_err(|e| {make_input_err!("Failed to parse {e} as uuid") })?,
        })
    }
}

impl std::fmt::Display for RedisOperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id_str = format!("{}:{}", self.unique_qualifier.action_name(), self.id);
        write!(f, "{id_str}")
    }
}

impl std::fmt::Debug for RedisOperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id_str = format!("{}:{}", self.unique_qualifier.action_name(), self.id);
        write!(f, "{id_str}")
    }
}

/// Unique id of worker.
#[derive(Serialize, Deserialize, Eq, PartialEq, Hash, Copy, Clone)]
pub struct WorkerId(pub Uuid);

impl std::fmt::Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut buf = Uuid::encode_buffer();
        let worker_id_str = self.0.hyphenated().encode_lower(&mut buf);
        write!(f, "{worker_id_str}")
    }
}

impl std::fmt::Debug for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut buf = Uuid::encode_buffer();
        let worker_id_str = self.0.hyphenated().encode_lower(&mut buf);
        f.write_str(worker_id_str)
    }
}

impl TryFrom<String> for WorkerId {
    type Error = Error;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        match Uuid::parse_str(&s) {
            Err(e) => Err(make_input_err!(
                "Failed to convert string to WorkerId : {} : {:?}",
                s,
                e
            )),
            Ok(my_uuid) => Ok(WorkerId(my_uuid)),
        }
    }
}
/// This is a utility struct used to make it easier to match `RedisActionInfos` in a
/// `HashMap` without needing to construct an entire `RedisActionInfo`.
/// Since the hashing only needs the digest and salt we can just alias them here
/// and point the original `RedisActionInfo` structs to reference these structs for
/// it's hashing functions.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RedisActionInfoHashKey {
    /// Name of instance group this action belongs to.
    pub instance_name: String,
    /// The digest function this action expects.
    pub digest_function: RedisDigestHasherFunc,
    /// Digest of the underlying `Action`.
    pub digest: RedisDigest,
    /// Salt that can be filled with a random number to ensure no `RedisActionInfo` will be a match
    /// to another `RedisActionInfo` in the scheduler. When caching is wanted this value is usually
    /// zero.
    pub salt: u64,
}

impl RedisActionInfoHashKey {
    /// Utility function used to make a unique hash of the digest including the salt.
    pub fn get_hash(&self) -> [u8; 32] {
        Blake3Hasher::new()
            .update(self.instance_name.as_bytes())
            .update(&i32::from(self.digest_function.proto_digest_func()).to_le_bytes())
            .update(&self.digest.packed_hash()[..])
            .update(&self.digest.size_bytes.to_le_bytes())
            .update(&self.salt.to_le_bytes())
            .finalize()
            .into()
    }

    /// Returns the salt used for cache busting/hashing.
    #[inline]
    pub fn action_name(&self) -> String {
        format!(
            "{}/{}/{}-{}/{:X}",
            self.instance_name,
            self.digest_function,
            self.digest.hash,
            self.digest.size_bytes,
            self.salt
        )
    }
}


/// Supported digest hash functions.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub enum RedisDigestHasherFunc {
    Sha256,
    Blake3,
}

impl RedisDigestHasherFunc {
    #[must_use]
    pub const fn proto_digest_func(&self) -> ProtoDigestFunction {
        match self {
            Self::Sha256 => ProtoDigestFunction::Sha256,
            Self::Blake3 => ProtoDigestFunction::Blake3,
        }
    }
}
impl From<DigestHasherFunc> for RedisDigestHasherFunc {
    fn from(value: DigestHasherFunc) -> Self {
        Self::try_from(value.proto_digest_func()).unwrap()
    }
}

impl TryFrom<i32> for RedisDigestHasherFunc {
    type Error = Error;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        // Zero means not-set.
        if value == 0 {
            return Ok(default_digest_hasher_func().into());
        }
        match ProtoDigestFunction::try_from(value) {
            Ok(ProtoDigestFunction::Sha256) => Ok(Self::Sha256),
            Ok(ProtoDigestFunction::Blake3) => Ok(Self::Blake3),
            value => Err(make_input_err!(
                "Unknown or unsupported digest function for int conversion: {:?}",
                value.map(|v| v.as_str_name())
            )),
        }
    }
}

impl TryFrom<ProtoDigestFunction> for RedisDigestHasherFunc {
    type Error = Error;

    fn try_from(value: ProtoDigestFunction) -> Result<Self, Self::Error> {
        match value {
            ProtoDigestFunction::Sha256 => Ok(Self::Sha256),
            ProtoDigestFunction::Blake3 => Ok(Self::Blake3),
            v => Err(make_input_err!(
                "Unknown or unsupported digest function for proto conversion {v:?}"
            )),
        }
    }
}

impl TryFrom<&str> for RedisDigestHasherFunc {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_uppercase().as_str() {
            "SHA256" => Ok(Self::Sha256),
            "BLAKE3" => Ok(Self::Blake3),
            v => Err(make_input_err!(
                "Unknown or unsupported digest function for string conversion: {v:?}"
            )),
        }
    }
}

impl std::fmt::Display for RedisDigestHasherFunc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RedisDigestHasherFunc::Sha256 => write!(f, "SHA256"),
            RedisDigestHasherFunc::Blake3 => write!(f, "BLAKE3"),
        }
    }
}


impl TryFrom<&str> for RedisActionInfoHashKey {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let (instance_name, other) = value
            .split_once('/')
            .err_tip(|| "Invalid RedisActionInfoHashKey string - {value}")?;
        let (digest_function, other) = other
            .split_once('/')
            .err_tip(|| "Invalid RedisActionInfoHashKey string - {value}")?;
        let (digest_hash, other) = other
            .split_once('-')
            .err_tip(|| "Invalid RedisActionInfoHashKey string - {value}")?;
        let (digest_size, salt) = other
            .split_once('/')
            .err_tip(|| "Invalid RedisActionInfoHashKey string - {value}")?;
        let digest = RedisDigest::try_new(
            digest_hash,
            digest_size
                .parse::<u64>()
                .err_tip(|| "Expected digest size to be a number for RedisActionInfoHashKey")?,
        )?;
        let salt = u64::from_str_radix(salt, 16).err_tip(|| "Expected salt to be a hex string")?;
        Ok(Self {
            instance_name: instance_name.to_string(),
            digest_function: digest_function.try_into()?,
            digest,
            salt,
        })
    }
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub enum RedisPlatformPropertyValue {
    Exact(String),
    Minimum(u64),
    Priority(String),
    Unknown(String)
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, Default)]
pub struct RedisPlatformProperties {
    pub properties: HashMap<String, RedisPlatformPropertyValue>,
}


/// Information needed to execute an action. This struct is used over bazel's proto `Action`
/// for simplicity and offers a `salt`, which is useful to ensure during hashing (for dicts)
/// to ensure we never match against another `RedisActionInfo` (when a task should never be cached).
/// This struct must be 100% compatible with `ExecuteRequest` struct in `remote_execution.proto`
/// except for the salt field.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RedisActionInfo {
    /// Digest of the underlying `Command`.
    pub command_digest: RedisDigest,
    /// Digest of the underlying `Directory`.
    pub input_root_digest: RedisDigest,
    /// Timeout of the action.
    pub timeout: Duration,
    /// The properties rules that must be applied when finding a worker that can run this action.
    pub platform_properties: RedisPlatformProperties,
    /// The priority of the action. Higher value means it should execute faster.
    pub priority: i32,
    /// When this action started to be loaded from the CAS.
    pub load_timestamp: SystemTime,
    /// When this action was created.
    pub insert_timestamp: SystemTime,

    /// Info used to uniquely identify this RedisActionInfo. Normally the hash function would just
    /// use the fields it needs and you wouldn't need to separate them, however we have a use
    /// case where we sometimes want to lookup an entry in a HashMap, but we don't have the
    /// info to construct an entire RedisActionInfo. In such case we construct only a RedisActionInfoHashKey
    /// then use that object to lookup the entry in the map. The root problem is that HashMap
    /// requires `RedisActionInfo :Borrow<RedisActionInfoHashKey>` in order for this to work, which means
    /// we need to be able to return a &RedisActionInfoHashKey from RedisActionInfo, but since we cannot
    /// return a temporary reference we must have an object tied to RedisActionInfo's lifetime and
    /// return it's reference.
    pub unique_qualifier: RedisActionInfoHashKey,

    /// Whether to try looking up this action in the cache.
    pub skip_cache_lookup: bool,
}

impl RedisActionInfo {
    #[inline]
    pub const fn instance_name(&self) -> &String {
        &self.unique_qualifier.instance_name
    }

    /// Returns the underlying digest of the `Action`.
    #[inline]
    pub const fn digest(&self) -> &RedisDigest {
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
            platform_properties: action.platform.unwrap_or_default().into(),
            priority: execute_request.execution_policy.unwrap_or_default().priority,
            load_timestamp,
            insert_timestamp: queued_timestamp,
            unique_qualifier: RedisActionInfoHashKey {
                instance_name: execute_request.instance_name,
                digest_function: RedisDigestHasherFunc::try_from(execute_request.digest_function)
                    .err_tip(|| format!("Could not find digest_function in try_from_action_and_execute_request_with_salt {:?}", execute_request.digest_function))?,
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

impl From<RedisActionInfo> for ExecuteRequest {
    fn from(val: RedisActionInfo) -> Self {
        let digest = val.digest().into();
        Self {
            instance_name: val.unique_qualifier.instance_name,
            action_digest: Some(digest),
            skip_cache_lookup: true, // The worker should never cache lookup.
            execution_policy: None,  // Not used in the worker.
            results_cache_policy: None, // Not used in the worker.
            digest_function: val
                .unique_qualifier
                .digest_function
                .proto_digest_func()
                .into(),
        }
    }
}

// Note: Hashing, Eq, and Ord matching on this struct is unique. Normally these functions
// must play well with each other, but in our case the following rules apply:
// * Hash - Hashing must be unique on the exact command being run and must never match
//          when do_not_cache is enabled, but must be consistent between identical data
//          hashes.
// * Eq   - Same as hash.
// * Ord  - Used when sorting `RedisActionInfo` together. The only major sorting is priority and
//          insert_timestamp, everything else is undefined, but must be deterministic.
impl Hash for RedisActionInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        RedisActionInfoHashKey::hash(&self.unique_qualifier, state);
    }
}

impl PartialEq for RedisActionInfo {
    fn eq(&self, other: &Self) -> bool {
        RedisActionInfoHashKey::eq(&self.unique_qualifier, &other.unique_qualifier)
    }
}

impl Eq for RedisActionInfo {}

impl Ord for RedisActionInfo {
    fn cmp(&self, other: &Self) -> Ordering {
        // Want the highest priority on top, but the lowest insert_timestamp.
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.insert_timestamp.cmp(&self.insert_timestamp))
            .then_with(|| self.salt().cmp(other.salt()))
            .then_with(|| self.digest().size_bytes.cmp(&other.digest().size_bytes))
            .then_with(|| self.digest().hash.cmp(&other.digest().hash))
            .then_with(|| {
                self.unique_qualifier
                    .digest_function
                    .cmp(&other.unique_qualifier.digest_function)
            })
    }
}

impl PartialOrd for RedisActionInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Borrow<RedisActionInfoHashKey> for Arc<RedisActionInfo> {
    #[inline]
    fn borrow(&self) -> &RedisActionInfoHashKey {
        &self.unique_qualifier
    }
}

impl Hash for RedisActionInfoHashKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Digest is unique, so hashing it is all we need.
        self.digest_function.hash(state);
        self.digest.hash(state);
        self.salt.hash(state);
    }
}

impl PartialEq for RedisActionInfoHashKey {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest
            && self.salt == other.salt
            && self.digest_function == other.digest_function
    }
}

impl Eq for RedisActionInfoHashKey {}

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
    pub digest: RedisDigest,
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
    pub tree_digest: RedisDigest,
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

impl Default for ExecutionMetadata {
    fn default() -> Self {
        Self {
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
    pub stdout_digest: RedisDigest,
    pub stderr_digest: RedisDigest,
    pub execution_metadata: ExecutionMetadata,
    pub server_logs: HashMap<String, RedisDigest>,
    pub error: Option<Error>,
    pub message: String,
}

impl Default for ActionResult {
    fn default() -> Self {
        ActionResult {
            output_files: Default::default(),
            output_folders: Default::default(),
            output_directory_symlinks: Default::default(),
            output_file_symlinks: Default::default(),
            exit_code: INTERNAL_ERROR_EXIT_CODE,
            stdout_digest: RedisDigest::zero_digest(),
            stderr_digest: RedisDigest::zero_digest(),
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
            error: None,
            message: String::new(),
        }
    }
}

// TODO(allada) Remove the need for clippy argument by making the ActionResult and ProtoActionResult
// a Box.
/// The execution status/stage. This should match `ExecutionStage::Value` in `remote_execution.proto`.
#[derive(PartialEq, Debug, Clone)]
#[allow(clippy::large_enum_variant)]
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
}

impl MetricsComponent for ActionStage {
    fn gather_metrics(&self, c: &mut CollectorState) {
        let (stage, maybe_exit_code) = match self {
            ActionStage::Unknown => ("Unknown", None),
            ActionStage::CacheCheck => ("CacheCheck", None),
            ActionStage::Queued => ("Queued", None),
            ActionStage::Executing => ("Executing", None),
            ActionStage::Completed(action_result) => ("Completed", Some(action_result.exit_code)),
            ActionStage::CompletedFromCache(proto_action_result) => {
                ("CompletedFromCache", Some(proto_action_result.exit_code))
            }
        };
        c.publish("stage", &stage.to_string(), "The state of the action.");
        if let Some(exit_code) = maybe_exit_code {
            c.publish("exit_code", &exit_code, "The exit code of the action.");
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
            ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) => Self::Completed,
        }
    }
}

pub fn to_execute_response(action_result: ActionResult) -> ExecuteResponse {
    fn logs_from(server_logs: HashMap<String, RedisDigest>) -> HashMap<String, LogFile> {
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
            .map_or_else(Status::default, |v| v.into()),
    );
    let message = action_result.message.clone();
    ExecuteResponse {
        server_logs: logs_from(action_result.server_logs.clone()),
        result: Some(action_result.into()),
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

impl From<ActionResult> for ProtoActionResult {
    fn from(val: ActionResult) -> Self {
        let mut output_symlinks = Vec::with_capacity(
            val.output_file_symlinks.len() + val.output_directory_symlinks.len(),
        );
        output_symlinks.extend_from_slice(val.output_file_symlinks.as_slice());
        output_symlinks.extend_from_slice(val.output_directory_symlinks.as_slice());

        Self {
            output_files: val.output_files.into_iter().map(Into::into).collect(),
            output_file_symlinks: val
                .output_file_symlinks
                .into_iter()
                .map(Into::into)
                .collect(),
            output_symlinks: output_symlinks.into_iter().map(Into::into).collect(),
            output_directories: val.output_folders.into_iter().map(Into::into).collect(),
            output_directory_symlinks: val
                .output_directory_symlinks
                .into_iter()
                .map(Into::into)
                .collect(),
            exit_code: val.exit_code,
            stdout_raw: Bytes::default(),
            stdout_digest: Some(val.stdout_digest.into()),
            stderr_raw: Bytes::default(),
            stderr_digest: Some(val.stderr_digest.into()),
            execution_metadata: Some(val.execution_metadata.into()),
        }
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
            server_logs: Default::default(),
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
            error: execute_response.status.clone().and_then(|v| {
                if v.code == 0 {
                    None
                } else {
                    Some(v.into())
                }
            }),
            message: execute_response.message,
        };

        if execute_response.cached_result {
            return Ok(Self::CompletedFromCache(action_result.into()));
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

impl TryFrom<Operation> for RedisActionState {
    type Error = Error;

    fn try_from(operation: Operation) -> Result<RedisActionState, Error> {
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

        // NOTE: This will error if we are forwarding an operation from
        // one remote execution system to another that does not use our operation name
        // format (ie: very unlikely, but possible).
        let id = RedisOperationId::try_from(operation.name.as_str())?;
        Ok(Self {
            id,
            stage,
        })
    }
}

/// Current state of the action.
/// This must be 100% compatible with `Operation` in `google/longrunning/operations.proto`.
#[derive(PartialEq, Debug, Clone)]
pub struct RedisActionState {
    pub stage: ActionStage,
    pub id: RedisOperationId
}


impl RedisActionState {
    #[inline]
    pub fn unique_qualifier(&self) -> &RedisActionInfoHashKey {
        &self.id.unique_qualifier
    }
    #[inline]
    pub fn action_digest(&self) -> &RedisDigest {
        &self.id.unique_qualifier.digest
    }
}

impl MetricsComponent for RedisActionState {
    fn gather_metrics(&self, c: &mut CollectorState) {
        c.publish("stage", &self.stage, "");
    }
}

impl From<RedisActionState> for Operation {
    fn from(val: RedisActionState) -> Self {
        let stage = Into::<execution_stage::Value>::into(&val.stage) as i32;
        let name = val.id.to_string();

        let result = if val.stage.has_action_result() {
            let execute_response: ExecuteResponse = val.stage.into();
            Some(LongRunningResult::Response(to_any(&execute_response)))
        } else {
            None
        };
        let digest = Some(val.id.unique_qualifier.digest.into());

        let metadata = ExecuteOperationMetadata {
            stage,
            action_digest: digest,
            // TODO(blaise.bruer) We should support stderr/stdout streaming.
            stdout_stream_name: String::default(),
            stderr_stream_name: String::default(),
            partial_execution_metadata: None,
        };

        Self {
            name,
            metadata: Some(to_any(&metadata)),
            done: result.is_some(),
            result,
        }
    }
}


#[derive(Serialize, Deserialize, Default, Clone, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct RedisDigest {
    /// Raw hash in packed form.
    pub hash: String,

    /// Possibly the size of the digest in bytes.
    pub size_bytes: i64,
}

impl RedisDigest {
    pub fn new(hash: &str, size_bytes: i64) -> Self {
        RedisDigest {
            size_bytes,
            hash: String::from(hash),
        }
    }

    pub fn from_bytes(packed_hash: &[u8; 32], size_bytes: i64) -> Self {
        RedisDigest {
            size_bytes,
            hash: hex::encode(packed_hash)
        }
    }

    pub fn packed_hash(&self) -> [u8; 32] {
        <[u8; 32]>::from_hex(&self.hash)
            .err_tip(|| format!("Invalid sha256 hash: {}", self.hash))
            .unwrap()
    }

    pub fn try_new<T>(hash: &str, size_bytes: T) -> Result<Self, Error>
    where
        T: TryInto<i64> + std::fmt::Display + Copy,
    {
        let size_bytes = size_bytes
            .try_into()
            .map_err(|_| make_input_err!("Could not convert {} into i64", size_bytes))?;
        Ok(RedisDigest {
            size_bytes,
            hash: hash.to_string(),
        })
    }

    pub fn zero_digest() -> RedisDigest {
        Self::from_bytes(&[0u8; 32], 0)
    }
}

impl fmt::Debug for RedisDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisDigest")
            .field("size_bytes", &self.size_bytes)
            .field("hash", &self.hash)
            .finish()
    }
}

impl Ord for RedisDigest {
    fn cmp(&self, other: &Self) -> Ordering {
        self.hash
            .cmp(&other.hash)
            .then_with(|| self.size_bytes.cmp(&other.size_bytes))
    }
}

impl PartialOrd for RedisDigest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl TryFrom<Digest> for RedisDigest {
    type Error = Error;

    fn try_from(digest: Digest) -> Result<Self, Self::Error> {
        Ok(RedisDigest {
            size_bytes: digest.size_bytes,
            hash: digest.hash,
        })
    }
}

impl TryFrom<&Digest> for RedisDigest {
    type Error = Error;

    fn try_from(digest: &Digest) -> Result<Self, Self::Error> {
        Ok(RedisDigest {
            size_bytes: digest.size_bytes,
            hash: digest.hash.to_owned(),
        })
    }
}

impl From<RedisDigest> for Digest {
    fn from(val: RedisDigest) -> Self {
        Digest {
            hash: val.hash,
            size_bytes: val.size_bytes,
        }
    }
}

impl From<&RedisDigest> for Digest {
    fn from(val: &RedisDigest) -> Self {
        Digest {
            hash: val.hash.to_string(),
            size_bytes: val.size_bytes,
        }
    }
}
