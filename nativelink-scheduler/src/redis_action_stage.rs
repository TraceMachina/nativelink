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

use nativelink_error::{make_input_err, Error};
use nativelink_util::action_messages::{ActionResult, ActionStage};
use serde::{Deserialize, Serialize};

use crate::operation_state_manager::OperationStageFlags;

// TODO(allada) Remove the need for clippy argument by making the ActionResult and ProtoActionResult
// a Box.
/// The execution status/stage. This should match `ExecutionStage::Value` in `remote_execution.proto`.
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum RedisOperationStage {
    CacheCheck,
    Queued,
    Executing,
    Completed(ActionResult),
    CompletedFromCache(ActionResult),
}

impl RedisOperationStage {
    pub fn as_state_flag(&self) -> OperationStageFlags {
        match self {
            Self::CacheCheck => OperationStageFlags::CacheCheck,
            Self::Executing => OperationStageFlags::Executing,
            Self::Queued => OperationStageFlags::Queued,
            Self::Completed(_) => OperationStageFlags::Completed,
            Self::CompletedFromCache(_) => OperationStageFlags::Completed,
        }
    }
}

impl TryFrom<ActionStage> for RedisOperationStage {
    type Error = Error;
    fn try_from(stage: ActionStage) -> Result<RedisOperationStage, Error> {
        match stage {
            ActionStage::CacheCheck => Ok(RedisOperationStage::CacheCheck),
            ActionStage::Queued => Ok(RedisOperationStage::Queued),
            ActionStage::Executing => Ok(RedisOperationStage::Executing),
            ActionStage::Unknown => Err(make_input_err!("ActionStage conversion to RedisOperationStage failed with Error - Unknown is not a valid OperationStage")),
            ActionStage::Completed(result) => Ok(RedisOperationStage::Completed(result)),
            ActionStage::CompletedFromCache(proto_result) => {
                let decoded = ActionResult::try_from(proto_result);
                match decoded {
                    Ok(result) => Ok(RedisOperationStage::Completed(result)),
                    Err(e) => Err(make_input_err!("ActionStage conversion to RedisOperationStage failed with Error - {e}"))
                }
            }
        }
    }
}

impl From<RedisOperationStage> for ActionStage {
    fn from(stage: RedisOperationStage) -> ActionStage {
        match stage {
            RedisOperationStage::CacheCheck => ActionStage::CacheCheck,
            RedisOperationStage::Queued => ActionStage::Queued,
            RedisOperationStage::Executing => ActionStage::Executing,
            RedisOperationStage::Completed(result) => ActionStage::Completed(result),
            RedisOperationStage::CompletedFromCache(result) => {
                ActionStage::CompletedFromCache(result.into())
            }
        }
    }
}

impl From<&RedisOperationStage> for ActionStage {
    fn from(stage: &RedisOperationStage) -> Self {
        match stage {
            RedisOperationStage::CacheCheck => ActionStage::CacheCheck,
            RedisOperationStage::Queued => ActionStage::Queued,
            RedisOperationStage::Executing => ActionStage::Executing,
            RedisOperationStage::Completed(result) => ActionStage::Completed(result.clone()),
            RedisOperationStage::CompletedFromCache(result) => {
                ActionStage::CompletedFromCache(result.clone().into())
            }
        }
    }
}
