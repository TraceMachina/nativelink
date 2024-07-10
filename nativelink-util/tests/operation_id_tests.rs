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
use nativelink_util::action_messages::OperationId;
use pretty_assertions::assert_eq;

#[nativelink_test]
async fn parse_operation_id() -> Result<(), Error> {
    {
        // Check no cached.
        let operation_id = OperationId::try_from("main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/u/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52").unwrap();
        assert_eq!(
            operation_id.to_string(),
            "main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/u/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52");
        assert_eq!(
            operation_id.id.to_string(),
            "19b16cf8-a1ad-4948-aaac-b6f4eb7fca52"
        );
    }
    {
        // Check cached.
        let operation_id = OperationId::try_from("main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/c/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52").unwrap();
        assert_eq!(
            operation_id.to_string(),
            "main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/c/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52");
        assert_eq!(
            operation_id.id.to_string(),
            "19b16cf8-a1ad-4948-aaac-b6f4eb7fca52"
        );
    }
    Ok(())
}

#[nativelink_test]
async fn parse_empty_failure() -> Result<(), Error> {
    let operation_id = OperationId::try_from("").err().unwrap();
    assert_eq!(operation_id.code, Code::Internal);
    assert_eq!(operation_id.messages.len(), 1);
    assert_eq!(
        operation_id.messages[0],
        "Invalid OperationId unique_qualifier / id fragment - "
    );

    let operation_id = OperationId::try_from("/").err().unwrap();
    assert_eq!(operation_id.code, Code::Internal);
    assert_eq!(operation_id.messages.len(), 1);
    assert_eq!(
        operation_id.messages[0],
        "Invalid UniqueQualifier instance name fragment - /"
    );

    let operation_id = OperationId::try_from("main").err().unwrap();
    assert_eq!(operation_id.code, Code::Internal);
    assert_eq!(operation_id.messages.len(), 1);
    assert_eq!(
        operation_id.messages[0],
        "Invalid OperationId unique_qualifier / id fragment - main"
    );

    let operation_id = OperationId::try_from("main/nohashfn/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/u/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52").err().unwrap();
    assert_eq!(operation_id.code, Code::InvalidArgument);
    assert_eq!(operation_id.messages.len(), 1);
    assert_eq!(
        operation_id.messages[0],
        "Unknown or unsupported digest function for string conversion: \"NOHASHFN\""
    );

    let operation_id =
        OperationId::try_from("main/SHA256/badhash-211/u/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52")
            .err()
            .unwrap();
    assert_eq!(operation_id.messages.len(), 3);
    assert_eq!(operation_id.code, Code::InvalidArgument);
    assert_eq!(operation_id.messages[0], "Odd number of digits");
    assert_eq!(operation_id.messages[1], "Invalid sha256 hash: badhash");
    assert_eq!(
        operation_id.messages[2],
        "Invalid DigestInfo digest hash - main/SHA256/badhash-211/u/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52"
    );

    let operation_id = OperationId::try_from("main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-/u/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52").err().unwrap();
    assert_eq!(operation_id.messages.len(), 2);
    assert_eq!(operation_id.code, Code::InvalidArgument);
    assert_eq!(
        operation_id.messages[0],
        "cannot parse integer from empty string"
    );
    assert_eq!(operation_id.messages[1], "Invalid UniqueQualifier size value fragment - main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-/u/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52");

    let operation_id = OperationId::try_from("main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f--211/u/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52").err().unwrap();
    assert_eq!(operation_id.code, Code::InvalidArgument);
    assert_eq!(operation_id.messages.len(), 2);
    assert_eq!(operation_id.messages[0], "invalid digit found in string");
    assert_eq!(operation_id.messages[1], "Invalid UniqueQualifier size value fragment - main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f--211/u/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52");

    let operation_id = OperationId::try_from("main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/x/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52").err().unwrap();
    assert_eq!(operation_id.messages.len(), 1);
    assert_eq!(operation_id.code, Code::InvalidArgument);
    assert_eq!(operation_id.messages[0], "Invalid UniqueQualifier cachable value fragment - main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/x/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52");

    let operation_id = OperationId::try_from("main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/-10/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52").err().unwrap();
    assert_eq!(operation_id.messages.len(), 1);
    assert_eq!(operation_id.code, Code::InvalidArgument);
    assert_eq!(operation_id.messages[0], "Invalid UniqueQualifier cachable value fragment - main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/-10/19b16cf8-a1ad-4948-aaac-b6f4eb7fca52");

    let operation_id = OperationId::try_from("main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/u/baduuid").err().unwrap();
    assert_eq!(operation_id.messages.len(), 1);
    assert_eq!(operation_id.code, Code::InvalidArgument);
    assert_eq!(operation_id.messages[0], "Failed to parse invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `u` at 4 as uuid");

    let operation_id = OperationId::try_from(
        "main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/0",
    )
    .err()
    .unwrap();
    assert_eq!(operation_id.messages.len(), 1);
    assert_eq!(operation_id.code, Code::Internal);
    assert_eq!(operation_id.messages[0], "Invalid UniqueQualifier digest size fragment - main/SHA256/4a0885a39d5ba8da3123c02ff56b73196a8b23fd3c835e1446e74a3a3ff4313f-211/0");

    Ok(())
}
