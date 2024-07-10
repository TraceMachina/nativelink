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

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nativelink_util::action_messages::{ActionInfo, ActionUniqueKey, ActionUniqueQualifier};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::platform_properties::PlatformProperties;

pub const INSTANCE_NAME: &str = "foobar_instance_name";

pub fn make_base_action_info(
    insert_timestamp: SystemTime,
    action_digest: DigestInfo,
) -> ActionInfo {
    ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: PlatformProperties {
            properties: HashMap::new(),
        },
        priority: 0,
        load_timestamp: UNIX_EPOCH,
        insert_timestamp,
        unique_qualifier: ActionUniqueQualifier::Cachable(ActionUniqueKey {
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    }
}
