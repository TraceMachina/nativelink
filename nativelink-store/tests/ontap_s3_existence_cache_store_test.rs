// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use core::time::Duration;
use std::sync::Arc;

use aws_sdk_s3::config::{BehaviorVersion, Builder, Region};
use aws_smithy_runtime::client::http::test_util::{ReplayEvent, StaticReplayClient};
use aws_smithy_types::body::SdkBody;
use bytes::Bytes;
use http::status::StatusCode;
use nativelink_config::stores::{
    CommonObjectSpec, ExperimentalOntapS3Spec, OntapS3ExistenceCacheSpec, Retry, StoreSpec,
};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::ontap_s3_existence_cache_store::OntapS3ExistenceCache;
use nativelink_store::ontap_s3_store::OntapS3Store;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::spawn;
use nativelink_util::store_trait::{Store, StoreLike};
use pretty_assertions::assert_eq;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const BUCKET_NAME: &str = "ontap-test-bucket";
const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
const VSERVER_NAME: &str = "testvserver";

async fn create_test_store(mock_client: StaticReplayClient) -> Result<Store, Error> {
    // Create a temporary directory for the cache file
    let temp_dir = tempdir().expect("Failed to create temporary directory");
    let cache_path = temp_dir
        .path()
        .join("cache_index.json")
        .to_str()
        .unwrap()
        .to_string();

    let _test_config = Builder::new()
        .behavior_version(BehaviorVersion::v2025_01_17())
        .region(Region::from_static(VSERVER_NAME))
        .http_client(mock_client)
        .build();

    let ontap_s3_spec = ExperimentalOntapS3Spec {
        endpoint: "https://example.com".to_string(),
        vserver_name: VSERVER_NAME.to_string(),
        bucket: BUCKET_NAME.to_string(),
        root_certificates: None,
        common: CommonObjectSpec {
            key_prefix: None,
            retry: Retry::default(),
            consider_expired_after_s: 0,
            max_retry_buffer_per_request: None,
            multipart_max_concurrent_uploads: None,
            insecure_allow_http: false,
            disable_http2: false,
        },
    };

    let cache_spec = OntapS3ExistenceCacheSpec {
        index_path: cache_path,
        sync_interval_seconds: 10,
        backend: Box::new(ontap_s3_spec),
    };

    let store_manager = Arc::new(StoreManager::new());
    store_factory(
        &StoreSpec::OntapS3ExistenceCache(Box::new(cache_spec)),
        &store_manager,
        None,
    )
    .await
}

#[nativelink_test]
async fn test_zero_digest_handling() -> Result<(), Error> {
    // Setup a mock client that doesn't expect any calls (zero digest is handled locally)
    let mock_client = StaticReplayClient::new(vec![]);

    let store = create_test_store(mock_client).await?;

    // Create the empty/zero digest
    let zero_digest = DigestInfo::new(Sha256::new().finalize().into(), 0);

    // has/exists check
    let result = store.has(zero_digest).await?;
    assert_eq!(result, Some(0), "Zero digest should exist with size 0");

    // get_part check
    let (mut writer, mut reader) = make_buf_channel_pair();
    let store_clone = store.clone();
    let _drop_guard = spawn!("zero_digest_test", async move {
        store_clone
            .get_part(zero_digest, &mut writer, 0, None)
            .await
            .unwrap();
    });

    let data = reader.consume(Some(1024)).await?;
    assert_eq!(data, Bytes::new(), "Zero digest should return empty data");

    Ok(())
}

#[nativelink_test]
async fn test_get_part_not_in_cache() -> Result<(), Error> {
    // Setup client with empty response
    let mock_client = StaticReplayClient::new(vec![
        // List objects response (empty)
        ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{VSERVER_NAME}.amazonaws.com/?list-type=2&max-keys=1000&prefix="
                ))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .body(
                    SdkBody::from(
                        r#"<?xml version="1.0" encoding="UTF-8"?>
                        <ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
                            <Name>ontap-test-bucket</Name>
                            <Prefix></Prefix>
                            <KeyCount>0</KeyCount>
                            <MaxKeys>1000</MaxKeys>
                            <IsTruncated>false</IsTruncated>
                        </ListBucketResult>"#,
                    )
                )
                .unwrap(),
        ),
        // Head object request (not found)
        ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{VSERVER_NAME}.amazonaws.com/{VALID_HASH1}-10?x-id=HeadObject"
                ))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(SdkBody::empty())
                .unwrap(),
        ),
    ]);

    let store = create_test_store(mock_client).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let test_digest = DigestInfo::try_new(VALID_HASH1, 10)?;

    // Try to get part - should fail with NotFound
    let result = store.get_part_unchunked(test_digest, 0, None).await;
    assert!(
        result.is_err(),
        "get_part should fail for object not in cache"
    );
    assert_eq!(
        result.unwrap_err().code,
        nativelink_error::Code::NotFound,
        "Error should be NotFound"
    );

    Ok(())
}
#[nativelink_test]
async fn test_cache_population() -> Result<(), Error> {
    // Setup a mock client that returns a list of objects
    let mock_client = StaticReplayClient::new(vec![
        // List objects response with some test objects
        ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{VSERVER_NAME}.amazonaws.com/?list-type=2&max-keys=1000&prefix="
                ))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .body(
                    SdkBody::from(
                        r#"<?xml version="1.0" encoding="UTF-8"?>
                        <ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
                            <Name>ontap-test-bucket</Name>
                            <Prefix></Prefix>
                            <KeyCount>2</KeyCount>
                            <MaxKeys>1000</MaxKeys>
                            <IsTruncated>false</IsTruncated>
                            <Contents>
                                <Key>0123456789abcdef000000000000000000010000000000000123456789abcdef-100</Key>
                                <LastModified>2023-01-01T00:00:00.000Z</LastModified>
                                <Size>100</Size>
                            </Contents>
                            <Contents>
                                <Key>0123456789abcdef000000000000000000020000000000000123456789abcdef-200</Key>
                                <LastModified>2023-01-01T00:00:00.000Z</LastModified>
                                <Size>200</Size>
                            </Contents>
                        </ListBucketResult>"#,
                    )
                )
                .unwrap(),
        ),
        // Head object request for the first object (when checking it exists)
        ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{VSERVER_NAME}.amazonaws.com/0123456789abcdef000000000000000000010000000000000123456789abcdef-100?x-id=HeadObject"
                ))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Length", "100")
                .body(SdkBody::empty())
                .unwrap(),
        ),
        // Head object request for the second object (we'll check it)
        ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{VSERVER_NAME}.amazonaws.com/0123456789abcdef000000000000000000020000000000000123456789abcdef-200?x-id=HeadObject"
                ))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Length", "200")
                .body(SdkBody::empty())
                .unwrap(),
        ),
    ]);

    let temp_dir = tempdir().expect("Failed to create temporary directory");
    let cache_path = temp_dir
        .path()
        .join("cache_index.json")
        .to_str()
        .unwrap()
        .to_string();

    let test_config = Builder::new()
        .behavior_version(BehaviorVersion::v2025_01_17())
        .region(Region::from_static(VSERVER_NAME))
        .http_client(mock_client.clone())
        .build();
    let s3_client = aws_sdk_s3::Client::from_conf(test_config);

    let ontap_s3_store = OntapS3Store::new_with_client_and_jitter(
        &(ExperimentalOntapS3Spec {
            bucket: BUCKET_NAME.to_string(),
            vserver_name: VSERVER_NAME.to_string(),
            endpoint: "https://example.com".to_string(),
            ..Default::default()
        }),
        s3_client.clone(),
        Arc::new(move |_delay| Duration::from_secs(0)),
        MockInstantWrapped::default,
    )?;

    let inner_store = Store::new(ontap_s3_store);

    let empty_digests = std::collections::HashSet::new();

    let existence_cache = OntapS3ExistenceCache::new_for_testing(
        inner_store,
        Arc::new(s3_client),
        cache_path.clone(),
        empty_digests,
        10,
        MockInstantWrapped::default,
    );

    let store = Store::new(existence_cache.clone());

    existence_cache.run_sync().await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify the first object was correctly added to the cache
    let digest1 = DigestInfo::try_new(
        "0123456789abcdef000000000000000000010000000000000123456789abcdef",
        100,
    )?;
    let result = store.has(digest1).await?;
    assert_eq!(
        result,
        Some(100),
        "Object should exist in cache after population"
    );

    // Verify the second object was parsed correctly
    let digest2 = DigestInfo::try_new(
        "0123456789abcdef000000000000000000020000000000000123456789abcdef",
        200,
    )?;

    let mut check_result = [None];
    store
        .has_with_results(&[digest2.into()], &mut check_result)
        .await?;
    let in_cache = check_result[0].is_some();

    assert!(in_cache, "Second object should be in the cache");

    // Check that the cache was persisted to disk
    let cache_content = tokio::fs::read_to_string(&cache_path).await?;
    assert!(
        cache_content.contains("0123456789abcdef000000000000000000010000000000000123456789abcdef"),
        "First digest should be persisted in cache file"
    );
    assert!(
        cache_content.contains("0123456789abcdef000000000000000000020000000000000123456789abcdef"),
        "Second digest should be persisted in cache file"
    );

    Ok(())
}

#[nativelink_test]
async fn test_cache_sync_multiple_objects() -> Result<(), Error> {
    // Setup a mock client with multiple objects
    let mock_client = StaticReplayClient::new(vec![
        // List objects response with multiple test objects
        ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{VSERVER_NAME}.amazonaws.com/?list-type=2&max-keys=1000&prefix="
                ))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .body(
                    SdkBody::from(
                        r#"<?xml version="1.0" encoding="UTF-8"?>
                        <ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
                            <Name>ontap-test-bucket</Name>
                            <Prefix></Prefix>
                            <KeyCount>3</KeyCount>
                            <MaxKeys>1000</MaxKeys>
                            <IsTruncated>false</IsTruncated>
                            <Contents>
                                <Key>0123456789abcdef000000000000000000010000000000000123456789abcdef-100</Key>
                                <LastModified>2023-01-01T00:00:00.000Z</LastModified>
                                <Size>100</Size>
                            </Contents>
                            <Contents>
                                <Key>0123456789abcdef000000000000000000020000000000000123456789abcdef-200</Key>
                                <LastModified>2023-01-01T00:00:00.000Z</LastModified>
                                <Size>200</Size>
                            </Contents>
                            <Contents>
                                <Key>0123456789abcdef000000000000000000030000000000000123456789abcdef-300</Key>
                                <LastModified>2023-01-01T00:00:00.000Z</LastModified>
                                <Size>300</Size>
                            </Contents>
                        </ListBucketResult>"#,
                    )
                )
                .unwrap(),
        ),
        // Head object requests for each object
        ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{VSERVER_NAME}.amazonaws.com/0123456789abcdef000000000000000000010000000000000123456789abcdef-100?x-id=HeadObject"
                ))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Length", "100")
                .body(SdkBody::empty())
                .unwrap(),
        ),
        ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{VSERVER_NAME}.amazonaws.com/0123456789abcdef000000000000000000020000000000000123456789abcdef-200?x-id=HeadObject"
                ))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Length", "200")
                .body(SdkBody::empty())
                .unwrap(),
        ),
        ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{VSERVER_NAME}.amazonaws.com/0123456789abcdef000000000000000000030000000000000123456789abcdef-300?x-id=HeadObject"
                ))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Length", "300")
                .body(SdkBody::empty())
                .unwrap(),
        ),
    ]);

    let temp_dir = tempdir().expect("Failed to create temporary directory");
    let cache_path = temp_dir
        .path()
        .join("cache_index.json")
        .to_str()
        .unwrap()
        .to_string();

    let test_config = Builder::new()
        .behavior_version(BehaviorVersion::v2025_01_17())
        .region(Region::from_static(VSERVER_NAME))
        .http_client(mock_client.clone())
        .build();
    let s3_client = aws_sdk_s3::Client::from_conf(test_config);

    let ontap_s3_store = OntapS3Store::new_with_client_and_jitter(
        &(ExperimentalOntapS3Spec {
            bucket: BUCKET_NAME.to_string(),
            vserver_name: VSERVER_NAME.to_string(),
            endpoint: "https://example.com".to_string(),
            ..Default::default()
        }),
        s3_client.clone(),
        Arc::new(move |_delay| Duration::from_secs(0)),
        MockInstantWrapped::default,
    )?;

    let existence_cache = OntapS3ExistenceCache::new_for_testing(
        Store::new(ontap_s3_store),
        Arc::new(s3_client),
        cache_path.clone(),
        std::collections::HashSet::new(),
        10,
        MockInstantWrapped::default,
    );

    // Manually trigger sync
    existence_cache.run_sync().await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let store = Store::new(existence_cache);

    // Verify all three objects were added to the cache
    let digests = [
        DigestInfo::try_new(
            "0123456789abcdef000000000000000000010000000000000123456789abcdef",
            100,
        )?,
        DigestInfo::try_new(
            "0123456789abcdef000000000000000000020000000000000123456789abcdef",
            200,
        )?,
        DigestInfo::try_new(
            "0123456789abcdef000000000000000000030000000000000123456789abcdef",
            300,
        )?,
    ];

    // Check existence of each digest
    let mut check_results = [None, None, None];
    store
        .has_with_results(&digests.map(Into::into), &mut check_results)
        .await?;

    // Verify sizes are correct
    assert_eq!(
        check_results.map(|r| r.unwrap_or(0)),
        [100, 200, 300],
        "Object sizes should match"
    );

    Ok(())
}
#[nativelink_test]
async fn test_empty_bucket_handling() -> Result<(), Error> {
    // Setup a mock client with an empty bucket listing
    let mock_client = StaticReplayClient::new(vec![
        ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{VSERVER_NAME}.amazonaws.com/?list-type=2&max-keys=1000&prefix="
                ))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .body(
                    SdkBody::from(
                        r#"<?xml version="1.0" encoding="UTF-8"?>
                        <ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
                            <Name>ontap-test-bucket</Name>
                            <Prefix></Prefix>
                            <KeyCount>0</KeyCount>
                            <MaxKeys>1000</MaxKeys>
                            <IsTruncated>false</IsTruncated>
                        </ListBucketResult>"#,
                    )
                )
                .unwrap(),
        ),
    ]);

    let store = create_test_store(mock_client).await?;

    // Wait for cache initialization
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Check that no objects exist
    let test_digest = DigestInfo::try_new(VALID_HASH1, 100)?;
    let result = store.has(test_digest).await?;

    assert_eq!(
        result, None,
        "Empty bucket should not add any objects to cache"
    );
    Ok(())
}
