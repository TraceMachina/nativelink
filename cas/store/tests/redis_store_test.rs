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

use redis::{ConnectionLike, RedisError};
use redis_store::RedisStore;
use redis_test::{MockCmd, MockRedisConnection};

fn my_exists<C: ConnectionLike>(conn: &mut C, key: &str) -> Result<bool, RedisError> {
    let exists: bool = redis::cmd("EXISTS").arg(key).query(conn)?;
    Ok(exists)
}

#[cfg(test)]
mod s3_store_tests {
    // #[tokio::test]
    // async fn test_redis_store() {
    //     // Create a new RedisStore with your test configuration.
    //     let store = Arc::new(RedisStore::new("localhost", 6379).await.unwrap());

    //     // Test the RedisStore using the same tests as the other stores.
    //     // You might need to adjust these tests based on the specifics of your RedisStore implementation.
    //     super::test_store(store).await;
    // }
    use super::*;
    use redis_test::MockCmd;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_new() {
        let mock_connection = MockRedisConnection::new(vec![]);
        let store = RedisStore::new(&mock_connection);
        assert!(store.is_ok());
    }

    #[tokio::test]
    async fn test_has_with_results() {
        let mock_connection = MockRedisConnection::new(vec![
            MockCmd::new(redis::cmd("EXISTS").arg("hash1"), Ok(true)),
            MockCmd::new(redis::cmd("EXISTS").arg("hash2"), Ok(false)),
        ]);
        let store = Arc::new(RedisStore::new(&mock_connection));
        let digests = vec![DigestInfo::new("hash1", 1), DigestInfo::new("hash2", 2)];
        let mut results = vec![None, None];
        let result = store.has_with_results(&digests, &mut results).await;
        assert!(result.is_ok());
        assert_eq!(results, vec![Some(1), None]);
    }

    #[tokio::test]
    async fn test_update() {
        let mock_connection = MockRedisConnection::new(vec![MockCmd::new(
            redis::cmd("SET").arg("hash").arg("test data"),
            Ok(()),
        )]);
        let store = Arc::new(RedisStore::new(&mock_connection));
        let digest = DigestInfo::new("hash", 1);
        let (reader, mut writer) = tokio::io::duplex(10);
        tokio::spawn(async move {
            writer.write_all(b"test data").await.unwrap();
        });
        let result = store.update(digest, reader, UploadSizeInfo::new(1)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_part_ref() {
        let mock_connection =
            MockRedisConnection::new(vec![MockCmd::new(redis::cmd("GET").arg("hash"), Ok("test data"))]);
        let store = Arc::new(RedisStore::new(&mock_connection));
        let digest = DigestInfo::new("hash", 1);
        let (mut reader, writer) = tokio::io::duplex(10);
        let result = store.get_part_ref(digest, &mut writer, 0, None).await;
        assert!(result.is_ok());
        // Add more assertions based on your expected results
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_my_exists() {
            let mut conn = setup_connection();
            let key = "test_key";

            let result = my_exists(&mut conn, key);

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), true);

            teardown_connection(conn);
        }
    }

    #[tokio::test]
    async fn test_size_partitioning_store_with_redis_store() {
        // Create a new RedisStore with your test configuration.
        let mut mock_connection = MockRedisConnection::new(vec![]);
        let redis_store = Arc::new(RedisStore::new(&mut mock_connection));

        // Create a new SizePartitioningStore with the RedisStore as the lower_store.
        let size_partitioning_store = Arc::new(SizePartitioningStore::new(
            256 * 1024,
            redis_store, /* upper_store goes here */
        ));

        // Test the SizePartitioningStore using the same tests as the other stores.
        super::test_store(size_partitioning_store).await;
    }

    #[tokio::test]
    async fn test_basic_operations() {
        let mut mock_connection = MockRedisConnection::new(vec![
            MockCmd::new(redis::cmd("SET").arg("key").arg("value"), Ok(())),
            MockCmd::new(redis::cmd("GET").arg("key"), Ok("value")),
        ]);
        let mut store = RedisStore::new(&mut mock_connection);
        store.put("key", "value").await.unwrap();
        assert_eq!(store.get("key").await.unwrap(), "value");
    }

    #[tokio::test]
    async fn test_error_handling() {
        let mut mock_connection = MockRedisConnection::new(vec![MockCmd::new(
            redis::cmd("GET").arg("nonexistent_key"),
            Err(RedisError::from((ErrorKind::TypeError, "Key does not exist"))),
        )]);
        let mut store = RedisStore::new(&mut mock_connection);
        assert!(store.get("nonexistent_key").await.is_err());
    }

    #[tokio::test]
    async fn test_concurrency() {
        let mock_connection = Arc::new(MockRedisConnection::new(vec![
            MockCmd::new(redis::cmd("SET").arg("key1").arg("value1"), Ok(())),
            MockCmd::new(redis::cmd("SET").arg("key2").arg("value2"), Ok(())),
        ]));
        let store = Arc::new(RedisStore::new(&mock_connection));
        let task1 = tokio::spawn(async move {
            store.put("key1", "value1").await.unwrap();
        });
        let task2 = tokio::spawn(async move {
            store.put("key2", "value2").await.unwrap();
        });
        tokio::try_join!(task1, task2).unwrap();
    }

    // #[tokio::test]
    // async fn test_persistence() {
    //     // This test might not be applicable as persistence would require a real Redis instance and cannot be tested with mocks.
    // }

    // #[tokio::test]
    // async fn test_eviction_policy() {
    //     // This test might not be applicable as eviction policy would require a real Redis instance and cannot be tested with mocks.
    // }

    // #[tokio::test]
    // async fn test_integration_with_size_partitioning_store() {
    //     // This test might not be applicable as it would require a real Redis instance and cannot be tested with mocks.
    // }

    #[tokio::test]
    async fn test_redis_store_operations() {
        let mut mock_connection = MockRedisConnection::new(vec![
            MockCmd::new(redis::cmd("SET").arg("test_key").arg("test_value"), Ok(())),
            MockCmd::new(redis::cmd("EXISTS").arg("test_key"), Ok(1)),
            MockCmd::new(redis::cmd("GET").arg("test_key"), Ok("test_value")),
        ]);

        let store = Arc::new(RedisStore::new(&mut mock_connection));

        let digest = DigestInfo::new("test_key", 10);
        let mut reader = DropCloserReadHalf::new("test_value".as_bytes());
        let size_info = UploadSizeInfo::new(10);

        // Test update operation
        store.update(digest.clone(), reader, size_info).await.unwrap();

        // Test has_with_results operation
        let mut results = vec![None];
        store.has_with_results(&[digest.clone()], &mut results).await.unwrap();
        assert_eq!(results[0], Some(10));

        // Test get_part_ref operation
        let mut writer = DropCloserWriteHalf::new(vec![]);
        store.get_part_ref(digest, &mut writer, 0, None).await.unwrap();
        assert_eq!(writer.into_inner(), "test_value".as_bytes());
    }
}
