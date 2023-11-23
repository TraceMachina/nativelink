// Copyright 2023 The Native Link Authors. All rights reserved.
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

use std::env;
use std::ffi::OsString;
use std::pin::Pin;
use std::sync::Arc;

use error::{Error, ResultExt};
use native_link_store::ac_utils::upload_file_to_store;
use native_link_store::memory_store::MemoryStore;
use native_link_util::common::{fs, DigestInfo};
use native_link_util::store_trait::Store;
use rand::{thread_rng, Rng};
use tokio::io::AsyncWriteExt;

/// Get temporary path from either `TEST_TMPDIR` or best effort temp directory if
/// not set.
async fn make_temp_path(data: &str) -> OsString {
    let dir = format!(
        "{}/{}",
        env::var("TEST_TMPDIR").unwrap_or(env::temp_dir().to_str().unwrap().to_string()),
        thread_rng().gen::<u64>(),
    );
    fs::create_dir_all(&dir).await.unwrap();
    OsString::from(format!("{}/{}", dir, data))
}

#[cfg(test)]
mod ac_utils_tests {
    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";
    const HASH1_SIZE: i64 = 147;

    // Regression test for bug created when implementing ResumeableFileSlot
    // where the timeout() success condition was breaking out of the outer
    // loop resulting in the file always being created with <= 4096 bytes.
    #[tokio::test]
    async fn upload_file_to_store_with_large_file() -> Result<(), Error> {
        let filepath = make_temp_path("test.txt").await;
        let expected_data = vec![0x88; 1024 * 1024]; // 1MB.
        let store = Arc::new(MemoryStore::new(&native_link_config::stores::MemoryStore::default()));
        let store_pin = Pin::new(store.as_ref());
        let digest = DigestInfo::try_new(HASH1, HASH1_SIZE)?; // Dummy hash data.
        {
            // Write 1MB of 0x88s to the file.
            let mut file = tokio::fs::File::create(&filepath)
                .await
                .err_tip(|| "Could not open file")?;
            file.write_all(&expected_data)
                .await
                .err_tip(|| "Could not write to file")?;
            file.flush().await.err_tip(|| "Could not flush file")?;
            file.sync_all().await.err_tip(|| "Could not sync file")?;
        }
        {
            // Upload our file.
            let resumeable_file = fs::open_file(filepath, u64::MAX).await?;
            upload_file_to_store(store_pin, digest, resumeable_file).await?;
        }
        {
            // Check to make sure the file was saved correctly to the store.
            let store_data = store_pin.get_part_unchunked(digest, 0, None, None).await?;
            assert_eq!(store_data.len(), expected_data.len());
            assert_eq!(store_data, expected_data);
        }
        Ok(())
    }
}
