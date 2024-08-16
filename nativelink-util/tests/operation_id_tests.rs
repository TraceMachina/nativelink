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

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_util::action_messages::OperationId;
use pretty_assertions::assert_eq;
use uuid::Uuid;

#[nativelink_test]
async fn parse_operation_id() -> Result<(), Error> {
    let operation_id = OperationId::from("19b16cf8-a1ad-4948-aaac-b6f4eb7fca52");
    assert_eq!(
        operation_id,
        OperationId::Uuid(Uuid::parse_str("19b16cf8-a1ad-4948-aaac-b6f4eb7fca52").unwrap())
    );
    Ok(())
}

#[nativelink_test]
async fn parse_empty_failure() -> Result<(), Error> {
    {
        let operation_id = OperationId::from("");
        assert_eq!(operation_id, OperationId::String(String::new()));
    }
    {
        let operation_id = OperationId::from("foobar");
        assert_eq!(operation_id, OperationId::String("foobar".to_string()));
    }

    Ok(())
}
