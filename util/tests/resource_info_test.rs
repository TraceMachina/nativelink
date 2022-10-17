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

use resource_info::ResourceInfo;

#[cfg(test)]
mod resource_info_tests {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    #[tokio::test]
    async fn with_resource_name_blobs_test() -> Result<(), Box<dyn std::error::Error>> {
        const RESOURCE_NAME: &str = "foo_bar/blobs/HASH-HERE/12345";
        let resource_info = ResourceInfo::new(RESOURCE_NAME)?;

        assert_eq!(resource_info.instance_name, "foo_bar");
        assert_eq!(resource_info.uuid, None);
        assert_eq!(resource_info.hash, "HASH-HERE");
        assert_eq!(resource_info.expected_size, 12345);

        Ok(())
    }

    #[tokio::test]
    async fn with_resource_name_uploads_test() -> Result<(), Box<dyn std::error::Error>> {
        const RESOURCE_NAME: &str = "foo_bar/uploads/UUID-HERE/blobs/HASH-HERE/12345";
        let resource_info = ResourceInfo::new(RESOURCE_NAME)?;

        assert_eq!(resource_info.instance_name, "foo_bar");
        assert_eq!(resource_info.uuid, Some("UUID-HERE"));
        assert_eq!(resource_info.hash, "HASH-HERE");
        assert_eq!(resource_info.expected_size, 12345);

        Ok(())
    }

    #[tokio::test]
    async fn without_resource_name_blobs_test() -> Result<(), Box<dyn std::error::Error>> {
        const RESOURCE_NAME: &str = "blobs/HASH-HERE/12345";
        let resource_info = ResourceInfo::new(RESOURCE_NAME)?;

        assert_eq!(resource_info.instance_name, "");
        assert_eq!(resource_info.uuid, None);
        assert_eq!(resource_info.hash, "HASH-HERE");
        assert_eq!(resource_info.expected_size, 12345);

        Ok(())
    }

    #[tokio::test]
    async fn without_resource_name_uploads_test() -> Result<(), Box<dyn std::error::Error>> {
        const RESOURCE_NAME: &str = "uploads/UUID-HERE/blobs/HASH-HERE/12345";
        let resource_info = ResourceInfo::new(RESOURCE_NAME)?;

        assert_eq!(resource_info.instance_name, "");
        assert_eq!(resource_info.uuid, Some("UUID-HERE"));
        assert_eq!(resource_info.hash, "HASH-HERE");
        assert_eq!(resource_info.expected_size, 12345);

        Ok(())
    }
}
