// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

use native_link_scheduler::action_messages::{ActionInfo, ActionInfoHashKey};
use native_link_scheduler::platform_property_manager::PlatformProperties;
use native_link_util::common::DigestInfo;
use native_link_util::digest_hasher::DigestHasherFunc;

pub const INSTANCE_NAME: &str = "foobar_instance_name";

pub fn make_base_action_info(insert_timestamp: SystemTime) -> ActionInfo {
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
        unique_qualifier: ActionInfoHashKey {
            instance_name: INSTANCE_NAME.to_string(),
            digest: DigestInfo::new([0u8; 32], 0),
            salt: 0,
        },
        skip_cache_lookup: false,
        digest_function: DigestHasherFunc::Sha256,
    }
}
