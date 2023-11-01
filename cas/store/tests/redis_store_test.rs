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

// use common::{log, DigestInfo};
// use error::{Code, Error, ResultExt};
// use redis::{ConnectionLike, RedisError};

// use s3_store::S3Store;
// use traits::UploadSizeInfo;

// fn my_exists<C: ConnectionLike>(conn: &mut C, key: &str) -> Result<bool, RedisError> {
//     let exists: bool = redis::cmd("EXISTS").arg(key).query(conn)?;
//     Ok(exists)
// }

#[cfg(test)]
mod redis_store_tests {
    //use super::*;
    // use config::stores::{FilesystemStore, S3Store};
    use std::pin::Pin;
    //use std::sync::Arc;
    //use error::{Error, ResultExt};
    use error::ResultExt;
    use traits::{StoreTrait, UploadSizeInfo};
    // use tokio::io::AsyncWriteExt;
    //use redis_test::{MockRedisConnection, MockCmd, IntoRedisCmdBytes};
    use redis_store::RedisStore;
    //use redis::Commands;
    use common::DigestInfo;
    //use async_fixed_buffer::AsyncFixedBuf;
    use buf_channel::{make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf};

    const HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
    //const HASH2: &str = "0123456789abcdef000000000000000000020000000000000123456789abcdef";
    //const VALUE1: &str = "123";
    //const VALUE2: &str = "321";

    // #[tokio::test]
    // async fn test_redis_store() {
    //     // Create a new RedisStore with your test configuration.
    //     let store = Arc::new(RedisStore::new(&config).await.unwrap());

    //     // Test the RedisStore using the same tests as the other stores.
    //     // You might need to adjust these tests based on the specifics of your RedisStore implementation.
    //     super::test_store(store).await;
    // }

    #[tokio::test]
    async fn test_new() {
        //let mock_connection = MockRedisConnection::new(vec![]);
        let config = config::stores::RedisStore {
            host: "localhost".to_string(),
            port: 6379,
            ..Default::default()
        };
        let store = RedisStore::new(&config).await;
        match &store {
            Ok(_) => println!("Store created successfully"),
            Err(e) => println!("Error: {:?}", e),
        }
        assert!(store.is_ok());
    }

    // #[tokio::test]
    // async fn valid_results_in_store() -> Result<(), Error> {
    //     let digest = DigestInfo::try_new(HASH1, VALUE1.len())?;
    //     let mut mock_connection = MockRedisConnection::new(vec![]);
    //     let cmd = redis::cmd("SET");
    //     let cmd1 = redis::cmd("EXISTS");
    //     MockCmd::new(cmd, Ok(VALUE1.to_string())).arg(HASH1.to_string()).arg(VALUE1.to_string()).execute_async(&mut mock_connection).await.unwrap();
    //     MockCmd::new(cmd1, Ok(true)).arg(HASH1.to_string()).execute_async(&mut mock_connection).await.unwrap();

    //     let config = config::stores::RedisStore {
    //         host: "localhost".to_string(),
    //         port: 6379,
    //         ..Default::default()
    //     };

    //     let store = Box::pin(RedisStore::new(&config).await?);

    //     // Shutdown the store (you need to implement this part based on your RedisStore implementation)

    //     // Check if the data still exists after shutdown
    //     let result = store.as_ref().has(digest).await;
    //     assert_eq!(
    //         result,
    //         Ok(Some(VALUE1.len())),
    //         "Expected to find item, got: {:?}",
    //         result
    //     );
    //     Ok(())
    // }

    // #[tokio::test]
    // async fn test_get_part_ref() {
    //     let mock_connection = MockRedisConnection::new(vec![
    //         MockCmd::new(redis::cmd("GET").arg("hash"), Ok("test data")),
    //     ]);
    //     let store = Arc::new(RedisStore::new(&config::stores::RedisStore::default()));
    //     let digest = DigestInfo::new(HASH1, 1);
    //     let (mut reader, writer) = tokio::io::duplex(10);
    //     let result = store.get_part_ref(digest, &mut writer, 0, None).await;
    //     assert!(result.is_ok());
    //     // Add more assertions based on your expected results
    // }

    // #[cfg(test)]
    // mod tests {use super::*;

    //     #[test]
    //     fn test_my_exists() {
    //         let mut conn = setup_connection();
    //         let key = "test_key";

    //         let result = my_exists(&mut conn, key);

    //         assert!(result.is_ok());
    //         assert_eq!(result.unwrap(), true);

    //         teardown_connection(conn);
    //     }
    // }

    // #[tokio::test]
    // async fn test_size_partitioning_store_with_redis_and_s3() {
    //     // Create a new RedisStore with your test configuration.
    //     let redis_store = Arc::new(RedisStore::new(&config::stores::RedisStore::default()));
    //     // Create a new S3Store with your test configuration.
    //     let s3_store = Arc::new(S3Store::new(&config::stores::S3Store::default()));

    //     // Create a new SizePartitioningStore with the RedisStore as the lower_store and S3Store as the upper_store.
    //     // Items smaller than the threshold (256 * 1024) will go into RedisStore, larger items will go into S3Store.
    //     let size_partitioning_store = Arc::new(SizePartitioningStore::new(256 * 1024, redis_store, s3_store));

    //     // Test the SizePartitioningStore using the same tests as the other stores.
    //     super::test_store(size_partitioning_store).await;
    // }

    // #[tokio::test]
    // async fn test_basic_operations() {
    //     let mut mock_connection = MockRedisConnection::new(vec![
    //         MockCmd::new(redis::cmd("SET").arg("key").arg("value"), Ok(())),
    //         MockCmd::new(redis::cmd("GET").arg("key"), Ok("value")),
    //     ]);
    //     let mut store = RedisStore::new("localhost", 6379);
    //     store.put("key", "value").await.unwrap();
    //     assert_eq!(store.get("key").await.unwrap(), "value");
    // }

    // #[tokio::test]
    // async fn test_error_handling() {
    //     let mut mock_connection = MockRedisConnection::new(vec![
    //         MockCmd::new(redis::cmd("GET").arg("nonexistent_key"), Err(RedisError::from((ErrorKind::TypeError, "Key does not exist")))),
    //     ]);
    //     let mut store = RedisStore::new("localhost", 6379);
    //     assert!(store.get("nonexistent_key").await.is_err());
    // }

    // #[tokio::test]
    // async fn test_concurrency() {
    //     let mock_connection = Arc::new(MockRedisConnection::new(vec![
    //         MockCmd::new(redis::cmd("SET").arg("key1").arg("value1"), Ok(())),
    //         MockCmd::new(redis::cmd("SET").arg("key2").arg("value2"), Ok(())),
    //     ]));
    //     let store = Arc::new(RedisStore::new("localhost", 6379));
    //     let task1 = tokio::spawn(async move { store.put("key1", "value1").await.unwrap(); });
    //     let task2 = tokio::spawn(async move { store.put("key2", "value2").await.unwrap(); });
    //     tokio::try_join!(task1, task2).unwrap();
    // }

    #[tokio::test]
    async fn test_redis_store_operations() {
        // let mut mock_connection = MockRedisConnection::new(vec![
        //     MockCmd::new(redis::cmd("SET").arg("test_key").arg("test_value"), Ok("ok")),
        //     MockCmd::new(redis::cmd("EXISTS").arg("test_key"), Ok(1)),
        //     MockCmd::new(redis::cmd("GET").arg("test_key"), Ok("test_value")),
        // ]);

        let config = config::stores::RedisStore {
            host: "localhost".to_string(),
            port: 6379,
            ..Default::default()
        };
        let store = match RedisStore::new(&config).await.err_tip(|| "Redis store could ") {
            Ok(store) => store,
            Err(e) => {
                eprintln!("Failed to create RedisStore: {:?}", e);
                return;
            }
        };

        let (mut writer, reader) = make_buf_channel_pair();

        let pinned_store = Pin::new(&store);

        let mut hash_array = [0u8; 32];
        for (i, c) in HASH1.as_bytes().chunks(2).enumerate() {
            hash_array[i] = u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap();
        }
        let digest = DigestInfo::new(hash_array, 10);
        let size_info: UploadSizeInfo = UploadSizeInfo::ExactSize(10);

        // Test update operation
        pinned_store.update(digest.clone(), reader, size_info).await.unwrap();
        //.err_tip( || "Could not update store.");

        // Test has_with_results operation
        let mut results = vec![None];
        pinned_store
            .has_with_results(&[digest.clone()], &mut results)
            .await
            .unwrap();
        assert_eq!(results[0], Some(10));

        // Test get_part_ref operation
        pinned_store.get_part_ref(digest, &mut writer, 0, None).await.unwrap();
        let bytes_written = writer.get_bytes_written();
        assert_eq!(bytes_written, "test_value".as_bytes().len() as u64);
    }
}
