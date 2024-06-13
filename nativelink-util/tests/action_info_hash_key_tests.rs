// Copyright 2024 The Native Link Authors. All rights reserved.
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

use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_util::action_messages::ActionInfoHashKey;
use pretty_assertions::assert_eq;

#[nativelink_test]
async fn parse_action_info_hash_key() -> Result<(), Error> {
    let action_info_hash_key = ActionInfoHashKey::try_from(
        "main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/0",
    )
    .unwrap();
    assert_eq!(action_info_hash_key.instance_name, "main");
    assert_eq!(
        hex::encode(action_info_hash_key.digest.packed_hash),
        "4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f"
    );
    assert_eq!(action_info_hash_key.digest.size_bytes, 211);
    assert_eq!(action_info_hash_key.salt, 0);
    assert_eq!(action_info_hash_key.digest_function.to_string(), "SHA256");
    assert_eq!(
        action_info_hash_key.action_name(),
        "main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/0"
    );
    assert_eq!(
        hex::encode(action_info_hash_key.get_hash()),
        "5a36f0db39e27667c4b91937cd29c1df8799ba468f2de6810c6865be05517644"
    );
    Ok(())
}

#[nativelink_test]
async fn parse_empty_failure() -> Result<(), Error> {
    let action_info_hash_key = ActionInfoHashKey::try_from("").err().unwrap();
    assert_eq!(action_info_hash_key.code, Code::Internal);
    assert_eq!(action_info_hash_key.messages.len(), 1);
    assert_eq!(
        action_info_hash_key.messages[0],
        "Invalid ActionInfoHashKey instance name fragment - "
    );

    let action_info_hash_key = ActionInfoHashKey::try_from(":").err().unwrap();
    assert_eq!(action_info_hash_key.messages.len(), 1);
    assert_eq!(action_info_hash_key.code, Code::Internal);
    assert_eq!(
        action_info_hash_key.messages[0],
        "Invalid ActionInfoHashKey instance name fragment - :"
    );

    let action_info_hash_key = ActionInfoHashKey::try_from("main").err().unwrap();
    assert_eq!(action_info_hash_key.messages.len(), 1);
    assert_eq!(action_info_hash_key.code, Code::Internal);
    assert_eq!(
        action_info_hash_key.messages[0],
        "Invalid ActionInfoHashKey instance name fragment - main"
    );

    let action_info_hash_key = ActionInfoHashKey::try_from(
        "main/nohashfn/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/0",
    )
    .err()
    .unwrap();
    assert_eq!(action_info_hash_key.messages.len(), 1);
    assert_eq!(action_info_hash_key.code, Code::InvalidArgument);
    assert_eq!(
        action_info_hash_key.messages[0],
        "Unknown or unsupported digest function for string conversion: \"NOHASHFN\""
    );

    let action_info_hash_key = ActionInfoHashKey::try_from("main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/0:19b16cf8-a1ad-4948-aaac-b6f4eb7fca52").err().unwrap();
    assert_eq!(action_info_hash_key.messages.len(), 2);
    assert_eq!(action_info_hash_key.code, Code::InvalidArgument);
    assert_eq!(
        action_info_hash_key.messages[0],
        "invalid digit found in string"
    );
    assert_eq!(
        action_info_hash_key.messages[1],
        "Invalid ActionInfoHashKey salt hex conversion - main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/0:19b16cf8-a1ad-4948-aaac-b6f4eb7fca52"
    );

    let action_info_hash_key = ActionInfoHashKey::try_from("main/SHA256/badhash-211/0")
        .err()
        .unwrap();
    assert_eq!(action_info_hash_key.messages.len(), 3);
    assert_eq!(action_info_hash_key.code, Code::InvalidArgument);
    assert_eq!(action_info_hash_key.messages[0], "Odd number of digits");
    assert_eq!(
        action_info_hash_key.messages[1],
        "Invalid sha256 hash: badhash"
    );
    assert_eq!(
        action_info_hash_key.messages[2],
        "Invalid DigestInfo digest hash - main/SHA256/badhash-211/0"
    );

    let action_info_hash_key = ActionInfoHashKey::try_from(
        "main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-/0",
    )
    .err()
    .unwrap();
    assert_eq!(action_info_hash_key.messages.len(), 2);
    assert_eq!(action_info_hash_key.code, Code::InvalidArgument);
    assert_eq!(
        action_info_hash_key.messages[0],
        "cannot parse integer from empty string"
    );
    assert_eq!(action_info_hash_key.messages[1], "Invalid ActionInfoHashKey size value fragment - main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-/0");

    let action_info_hash_key = ActionInfoHashKey::try_from(
        "main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f--211/0",
    )
    .err()
    .unwrap();
    assert_eq!(action_info_hash_key.code, Code::InvalidArgument);
    assert_eq!(action_info_hash_key.messages.len(), 2);
    assert_eq!(
        action_info_hash_key.messages[0],
        "invalid digit found in string"
    );
    assert_eq!(action_info_hash_key.messages[1], "Invalid ActionInfoHashKey size value fragment - main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f--211/0");

    let action_info_hash_key = ActionInfoHashKey::try_from(
        "main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/x",
    )
    .err()
    .unwrap();
    assert_eq!(action_info_hash_key.messages.len(), 2);
    assert_eq!(action_info_hash_key.code, Code::InvalidArgument);
    assert_eq!(
        action_info_hash_key.messages[0],
        "invalid digit found in string"
    );
    assert_eq!(action_info_hash_key.messages[1], "Invalid ActionInfoHashKey salt hex conversion - main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/x");

    let action_info_hash_key = ActionInfoHashKey::try_from(
        "main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/-10",
    )
    .err()
    .unwrap();
    assert_eq!(action_info_hash_key.messages.len(), 2);
    assert_eq!(action_info_hash_key.code, Code::InvalidArgument);
    assert_eq!(
        action_info_hash_key.messages[0],
        "invalid digit found in string"
    );
    assert_eq!(action_info_hash_key.messages[1], "Invalid ActionInfoHashKey salt hex conversion - main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/-10");

    Ok(())
}
