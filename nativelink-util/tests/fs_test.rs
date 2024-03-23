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

use std::env;
use std::ffi::OsString;
use std::io::SeekFrom;
use std::str::from_utf8;

use nativelink_error::Error;
use nativelink_util::common::fs;
use rand::{thread_rng, Rng};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Semaphore;

/// Get temporary path from either `TEST_TMPDIR` or best effort temp directory if
/// not set.
async fn make_temp_path(data: &str) -> OsString {
    let dir = format!(
        "{}/{}",
        env::var("TEST_TMPDIR").unwrap_or(env::temp_dir().to_str().unwrap().to_string()),
        thread_rng().gen::<u64>(),
    );
    fs::create_dir_all(&dir).await.unwrap();
    OsString::from(format!("{dir}/{data}"))
}

static TEST_EXCLUSIVE_SEMAPHORE: Semaphore = Semaphore::const_new(1);

#[cfg(test)]
mod fs_tests {
    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    #[tokio::test]
    async fn resumeable_file_slot_write_close_write_test() -> Result<(), Error> {
        let _permit = TEST_EXCLUSIVE_SEMAPHORE.acquire().await; // One test at a time.
        let filename = make_temp_path("test_file.txt").await;
        {
            let mut file = fs::create_file(&filename).await?;
            file.as_writer().await?.write_all(b"Hello").await?;
            file.close_file().await?;
            assert_eq!(fs::get_open_files_for_test(), 0);
            file.as_writer().await?.write_all(b"Goodbye").await?;
            assert_eq!(fs::get_open_files_for_test(), 1);
            file.as_writer().await?.as_mut().sync_all().await?;
        }
        assert_eq!(fs::get_open_files_for_test(), 0);
        {
            let mut file = fs::open_file(&filename, u64::MAX).await?;
            let mut contents = String::new();
            file.as_reader()
                .await?
                .read_to_string(&mut contents)
                .await?;
            assert_eq!(contents, "HelloGoodbye");
        }
        Ok(())
    }

    #[tokio::test]
    async fn resumeable_file_slot_read_close_read_test() -> Result<(), Error> {
        let _permit = TEST_EXCLUSIVE_SEMAPHORE.acquire().await; // One test at a time.
        const DUMMYDATA: &str = "DummyDataTest";
        let filename = make_temp_path("test_file.txt").await;
        {
            let mut file = fs::create_file(&filename).await?;
            file.as_writer()
                .await?
                .write_all(DUMMYDATA.as_bytes())
                .await?;
            file.as_writer().await?.as_mut().sync_all().await?;
        }
        {
            let mut file = fs::open_file(&filename, u64::MAX).await?;
            let mut contents = [0u8; 5];
            {
                assert_eq!(file.as_reader().await?.read(&mut contents).await?, 5);
                assert_eq!(from_utf8(&contents[..]).unwrap(), "Dummy");
            }
            file.close_file().await?;
            {
                assert_eq!(file.as_reader().await?.read(&mut contents).await?, 5);
                assert_eq!(from_utf8(&contents[..]).unwrap(), "DataT");
            }
            file.close_file().await?;
            {
                assert_eq!(file.as_reader().await?.read(&mut contents).await?, 3);
                assert_eq!(from_utf8(&contents[..3]).unwrap(), "est");
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn resumeable_file_slot_read_close_read_with_take_test() -> Result<(), Error> {
        let _permit = TEST_EXCLUSIVE_SEMAPHORE.acquire().await; // One test at a time.
        const DUMMYDATA: &str = "DummyDataTest";
        let filename = make_temp_path("test_file.txt").await;
        {
            let mut file = fs::create_file(&filename).await?;
            file.as_writer()
                .await?
                .write_all(DUMMYDATA.as_bytes())
                .await?;
            file.as_writer().await?.as_mut().sync_all().await?;
        }
        {
            let mut file = fs::open_file(&filename, 11).await?;
            let mut contents = [0u8; 5];
            {
                assert_eq!(file.as_reader().await?.read(&mut contents).await?, 5);
                assert_eq!(from_utf8(&contents[..]).unwrap(), "Dummy");
            }
            assert_eq!(fs::get_open_files_for_test(), 1);
            file.close_file().await?;
            assert_eq!(fs::get_open_files_for_test(), 0);
            {
                assert_eq!(file.as_reader().await?.read(&mut contents).await?, 5);
                assert_eq!(from_utf8(&contents[..]).unwrap(), "DataT");
            }
            assert_eq!(fs::get_open_files_for_test(), 1);
            file.close_file().await?;
            assert_eq!(fs::get_open_files_for_test(), 0);
            {
                assert_eq!(file.as_reader().await?.read(&mut contents).await?, 1);
                assert_eq!(from_utf8(&contents[..1]).unwrap(), "e");
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn resumeable_file_slot_read_close_read_with_take_and_seek_test() -> Result<(), Error> {
        let _permit = TEST_EXCLUSIVE_SEMAPHORE.acquire().await; // One test at a time.
        const DUMMYDATA: &str = "DummyDataTest";
        let filename = make_temp_path("test_file.txt").await;
        {
            let mut file = fs::create_file(&filename).await?;
            file.as_writer()
                .await?
                .write_all(DUMMYDATA.as_bytes())
                .await?;
            file.as_writer().await?.as_mut().sync_all().await?;
        }
        {
            let mut file = fs::open_file(&filename, 11).await?;
            file.as_reader()
                .await?
                .get_mut()
                .seek(SeekFrom::Start(2))
                .await?;
            let mut contents = [0u8; 5];
            {
                assert_eq!(file.as_reader().await?.read(&mut contents).await?, 5);
                assert_eq!(from_utf8(&contents[..]).unwrap(), "mmyDa");
            }
            assert_eq!(fs::get_open_files_for_test(), 1);
            file.close_file().await?;
            assert_eq!(fs::get_open_files_for_test(), 0);
            {
                assert_eq!(file.as_reader().await?.read(&mut contents).await?, 5);
                assert_eq!(from_utf8(&contents[..]).unwrap(), "taTes");
            }
            file.close_file().await?;
            assert_eq!(fs::get_open_files_for_test(), 0);
            {
                assert_eq!(file.as_reader().await?.read(&mut contents).await?, 1);
                assert_eq!(from_utf8(&contents[..1]).unwrap(), "t");
            }
        }
        Ok(())
    }
}
