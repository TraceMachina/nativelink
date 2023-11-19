// Copyright 2022 The Native Link Authors. All rights reserved.
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

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use aws_sdk_s3::config::{Builder, Region};
use aws_smithy_runtime::client::http::test_util::{ReplayEvent, StaticReplayClient};
use aws_smithy_types::body::SdkBody;
use common::DigestInfo;
use error::Error;
use http::header;
use http::status::StatusCode;
use s3_store::S3Store;
use traits::StoreTrait;

// TODO(aaronmondal): Figure out how to test the connector retry mechanism.

#[cfg(test)]
mod s3_store_tests {
    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    const BUCKET_NAME: &str = "dummy-bucket-name";
    const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
    const REGION: &str = "testregion";

    #[tokio::test]
    async fn simple_has_object_found() -> Result<(), Error> {
        let mock_client = StaticReplayClient::new(vec![ReplayEvent::new(
            http::Request::builder().body(SdkBody::empty()).unwrap(),
            http::Response::builder()
                .header(header::CONTENT_LENGTH, "512")
                .body(SdkBody::empty())
                .unwrap(),
        )]);
        let test_config = Builder::new()
            .region(Region::from_static(REGION))
            .http_client(mock_client)
            .build();
        let s3_client = aws_sdk_s3::Client::from_conf(test_config);
        let store = S3Store::new_with_client_and_jitter(
            &config::stores::S3Store {
                bucket: BUCKET_NAME.to_string(),
                ..Default::default()
            },
            s3_client,
            Arc::new(move |_delay| Duration::from_secs(0)),
        )?;
        let store_pin = Pin::new(&store);

        let digest = DigestInfo::try_new(VALID_HASH1, 100).unwrap();
        let result = store_pin.has(digest).await;
        assert_eq!(result, Ok(Some(512)), "Expected to find item, got: {result:?}");
        Ok(())
    }

    #[tokio::test]
    async fn simple_has_object_not_found() -> Result<(), Error> {
        let mock_client = StaticReplayClient::new(vec![ReplayEvent::new(
            http::Request::builder().body(SdkBody::empty()).unwrap(),
            http::Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(SdkBody::empty())
                .unwrap(),
        )]);
        let test_config = Builder::new()
            .region(Region::from_static(REGION))
            .http_client(mock_client)
            .build();
        let s3_client = aws_sdk_s3::Client::from_conf(test_config);
        let store = S3Store::new_with_client_and_jitter(
            &config::stores::S3Store {
                bucket: BUCKET_NAME.to_string(),
                ..Default::default()
            },
            s3_client,
            Arc::new(move |_delay| Duration::from_secs(0)),
        )?;
        let store_pin = Pin::new(&store);
        let digest = DigestInfo::try_new(VALID_HASH1, 100).unwrap();
        let result = store_pin.has(digest).await;
        assert_eq!(result, Ok(None), "Expected to not find item, got: {result:?}");
        Ok(())
    }

    #[tokio::test]
    async fn simple_has_retries() -> Result<(), Error> {
        let mock_client = StaticReplayClient::new(vec![
            ReplayEvent::new(
                http::Request::builder().body(SdkBody::empty()).unwrap(),
                http::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
            ReplayEvent::new(
                http::Request::builder().body(SdkBody::empty()).unwrap(),
                http::Response::builder()
                    .status(StatusCode::SERVICE_UNAVAILABLE)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
            ReplayEvent::new(
                http::Request::builder().body(SdkBody::empty()).unwrap(),
                http::Response::builder()
                    .status(StatusCode::CONFLICT)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
            ReplayEvent::new(
                http::Request::builder().body(SdkBody::empty()).unwrap(),
                http::Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, "111")
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
        ]);
        let test_config = Builder::new()
            .region(Region::from_static(REGION))
            .http_client(mock_client)
            .build();
        let s3_client = aws_sdk_s3::Client::from_conf(test_config);

        let store = S3Store::new_with_client_and_jitter(
            &config::stores::S3Store {
                bucket: BUCKET_NAME.to_string(),
                retry: config::stores::Retry {
                    max_retries: 1024,
                    delay: 0.,
                    jitter: 0.,
                },
                ..Default::default()
            },
            s3_client,
            Arc::new(move |_delay| Duration::from_secs(0)),
        )?;
        let store_pin = Pin::new(&store);

        let digest = DigestInfo::try_new(VALID_HASH1, 100).unwrap();
        let result = store_pin.has(digest).await;
        assert_eq!(result, Ok(Some(111)), "Expected to find item, got: {:?}", result);
        Ok(())
    }

    #[tokio::test]
    async fn simple_update_ac() -> Result<(), Error> {
        const AC_ENTRY_SIZE: u64 = 199;
        const CONTENT_LENGTH: usize = 50;
        let mut send_data = Vec::with_capacity(CONTENT_LENGTH);
        for i in 0..send_data.capacity() {
            send_data.push(((i * 3) % 256) as u8);
        }

        let mock_client = StaticReplayClient::new(vec![ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{REGION}.amazonaws.com/{VALID_HASH1}-{AC_ENTRY_SIZE}?x-id=PutObject",
                ))
                .method("PUT")
                .header("content-type", "application/octet-stream")
                .header("content-length", CONTENT_LENGTH.to_string())
                .body(SdkBody::from(send_data.clone()))
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .body(SdkBody::empty())
                .unwrap(),
        )]);
        let test_config = Builder::new()
            .region(Region::from_static(REGION))
            .http_client(mock_client.clone())
            .build();
        let s3_client = aws_sdk_s3::Client::from_conf(test_config);
        let store = S3Store::new_with_client_and_jitter(
            &config::stores::S3Store {
                bucket: BUCKET_NAME.to_string(),
                ..Default::default()
            },
            s3_client,
            Arc::new(move |_delay| Duration::from_secs(0)),
        )?;
        let store_pin = Pin::new(&store);
        store_pin
            .update_oneshot(
                DigestInfo::try_new(VALID_HASH1, AC_ENTRY_SIZE)?,
                send_data.clone().into(),
            )
            .await?;
        mock_client.assert_requests_match(&[]);
        Ok(())
    }

    #[tokio::test]
    async fn simple_get_ac() -> Result<(), Error> {
        const VALUE: &str = "23";
        const AC_ENTRY_SIZE: u64 = 1000; // Any size that is not VALUE.len().

        let mock_client = StaticReplayClient::new(vec![ReplayEvent::new(
            http::Request::builder().body(SdkBody::empty()).unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .body(SdkBody::from(VALUE))
                .unwrap(),
        )]);
        let test_config = Builder::new()
            .region(Region::from_static(REGION))
            .http_client(mock_client)
            .build();
        let s3_client = aws_sdk_s3::Client::from_conf(test_config);
        let store = S3Store::new_with_client_and_jitter(
            &config::stores::S3Store {
                bucket: BUCKET_NAME.to_string(),
                ..Default::default()
            },
            s3_client,
            Arc::new(move |_delay| Duration::from_secs(0)),
        )?;
        let store_pin = Pin::new(&store);

        let store_data = store_pin
            .get_part_unchunked(DigestInfo::try_new(VALID_HASH1, AC_ENTRY_SIZE)?, 0, None, None)
            .await?;
        assert_eq!(
            store_data,
            VALUE.as_bytes(),
            "Hash for key: {VALID_HASH1} did not insert. Expected: {VALUE:#x?}, but got: {store_data:#x?}",
        );
        Ok(())
    }

    #[tokio::test]
    async fn smoke_test_get_part() -> Result<(), Error> {
        const AC_ENTRY_SIZE: u64 = 1000; // Any size that is not raw_send_data.len().
        const OFFSET: usize = 105;
        const LENGTH: usize = 50_000; // Just a size that is not the same as the real data size.
        let mock_client = StaticReplayClient::new(vec![ReplayEvent::new(
            http::Request::builder()
                .uri(format!(
                    "https://{BUCKET_NAME}.s3.{REGION}.amazonaws.com/{VALID_HASH1}-{AC_ENTRY_SIZE}?x-id=GetObject",
                ))
                .header("range", format!("bytes={}-{}", OFFSET, OFFSET + LENGTH))
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .body(SdkBody::empty())
                .unwrap(),
        )]);
        let test_config = Builder::new()
            .region(Region::from_static(REGION))
            .http_client(mock_client.clone())
            .build();
        let s3_client = aws_sdk_s3::Client::from_conf(test_config);
        let store = S3Store::new_with_client_and_jitter(
            &config::stores::S3Store {
                bucket: BUCKET_NAME.to_string(),
                ..Default::default()
            },
            s3_client,
            Arc::new(move |_delay| Duration::from_secs(0)),
        )?;
        let store_pin = Pin::new(&store);

        store_pin
            .get_part_unchunked(
                DigestInfo::try_new(VALID_HASH1, AC_ENTRY_SIZE)?,
                OFFSET,
                Some(LENGTH),
                None,
            )
            .await?;

        mock_client.assert_requests_match(&[]);
        Ok(())
    }

    #[tokio::test]
    async fn get_part_simple_retries() -> Result<(), Error> {
        let mock_client = StaticReplayClient::new(vec![
            ReplayEvent::new(
                http::Request::builder().body(SdkBody::empty()).unwrap(),
                http::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
            ReplayEvent::new(
                http::Request::builder().body(SdkBody::empty()).unwrap(),
                http::Response::builder()
                    .status(StatusCode::SERVICE_UNAVAILABLE)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
            ReplayEvent::new(
                http::Request::builder().body(SdkBody::empty()).unwrap(),
                http::Response::builder()
                    .status(StatusCode::CONFLICT)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
            ReplayEvent::new(
                http::Request::builder().body(SdkBody::empty()).unwrap(),
                http::Response::builder()
                    .status(StatusCode::OK)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
        ]);
        let test_config = Builder::new()
            .region(Region::from_static(REGION))
            .http_client(mock_client)
            .build();
        let s3_client = aws_sdk_s3::Client::from_conf(test_config);

        let store = S3Store::new_with_client_and_jitter(
            &config::stores::S3Store {
                bucket: BUCKET_NAME.to_string(),
                retry: config::stores::Retry {
                    max_retries: 1024,
                    delay: 0.,
                    jitter: 0.,
                },
                ..Default::default()
            },
            s3_client,
            Arc::new(move |_delay| Duration::from_secs(0)),
        )?;

        let digest = DigestInfo::try_new(VALID_HASH1, 100).unwrap();
        let result = Pin::new(&store).get_part_unchunked(digest, 0, None, None).await;
        assert!(result.is_ok(), "Expected to find item, got: {result:?}");
        Ok(())
    }

    #[tokio::test]
    async fn multipart_update_large_cas() -> Result<(), Error> {
        // Same as in s3_store.
        const MIN_MULTIPART_SIZE: usize = 5 * 1024 * 1024; // 5mb.
        const AC_ENTRY_SIZE: usize = MIN_MULTIPART_SIZE * 2 + 50;

        let mut send_data = Vec::with_capacity(AC_ENTRY_SIZE);
        for i in 0..send_data.capacity() {
            send_data.push(((i * 3) % 256) as u8);
        }
        let digest = DigestInfo::try_new(VALID_HASH1, send_data.len())?;

        let mock_client = StaticReplayClient::new(vec![
            ReplayEvent::new(
                http::Request::builder()
                    .uri(format!(
                        "https://{BUCKET_NAME}.s3.{REGION}.amazonaws.com/{VALID_HASH1}-{AC_ENTRY_SIZE}?uploads&x-id=CreateMultipartUpload",
                    ))
                    .method("POST")
                    .body(SdkBody::empty())
                    .unwrap(),
                http::Response::builder()
                    .status(StatusCode::OK)
                    .body(SdkBody::from(
                        r#"
                        <InitiateMultipartUploadResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
                          <UploadId>Dummy-uploadid</UploadId>
                        </InitiateMultipartUploadResult>"#
                            .as_bytes(),
                    ))
                    .unwrap(),
            ),
            ReplayEvent::new(
                http::Request::builder()
                    .uri(format!(
                        "https://{BUCKET_NAME}.s3.{REGION}.amazonaws.com/{VALID_HASH1}-{AC_ENTRY_SIZE}?x-id=UploadPart&partNumber=1&uploadId=Dummy-uploadid",
                    ))
                    .method("PUT")
                    .header("content-type", "application/octet-stream")
                    .header("content-length", "5242880")
                    .body(SdkBody::from(&send_data[0..MIN_MULTIPART_SIZE]))
                    .unwrap(),
                http::Response::builder()
                    .status(StatusCode::OK)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
            ReplayEvent::new(
                http::Request::builder()
                    .uri(format!(
                        "https://{BUCKET_NAME}.s3.{REGION}.amazonaws.com/{VALID_HASH1}-{AC_ENTRY_SIZE}?x-id=UploadPart&partNumber=2&uploadId=Dummy-uploadid",
                    ))
                    .method("PUT")
                    .header("content-type", "application/octet-stream")
                    .header("content-length", "5242880")
                    .body(SdkBody::from(&send_data[MIN_MULTIPART_SIZE..MIN_MULTIPART_SIZE * 2]))
                    .unwrap(),
                http::Response::builder()
                    .status(StatusCode::OK)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
            ReplayEvent::new(
                http::Request::builder()
                    .uri(format!(
                        "https://{BUCKET_NAME}.s3.{REGION}.amazonaws.com/{VALID_HASH1}-{AC_ENTRY_SIZE}?x-id=UploadPart&partNumber=3&uploadId=Dummy-uploadid",
                    ))
                    .method("PUT")
                    .header("content-type", "application/octet-stream")
                    .header("content-length", "50")
                    .body(SdkBody::from(
                        &send_data[MIN_MULTIPART_SIZE * 2..MIN_MULTIPART_SIZE * 2 + 50],
                    ))
                    .unwrap(),
                http::Response::builder()
                    .status(StatusCode::OK)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
            ReplayEvent::new(
                http::Request::builder()
                    .uri(format!(
                        "https://{BUCKET_NAME}.s3.{REGION}.amazonaws.com/{VALID_HASH1}-{AC_ENTRY_SIZE}?x-id=CompleteMultipartUpload&uploadId=Dummy-uploadid",
                    ))
                    .method("POST")
                    .header("content-length", "216")
                    .body(SdkBody::from(concat!(
                        r#"<CompleteMultipartUpload xmlns="http://s3.amazonaws.com/doc/2006-03-01/">"#,
                        "<Part><PartNumber>1</PartNumber></Part>",
                        "<Part><PartNumber>2</PartNumber></Part>",
                        "<Part><PartNumber>3</PartNumber></Part>",
                        "</CompleteMultipartUpload>",
                    )))
                    .unwrap(),
                http::Response::builder()
                    .status(StatusCode::OK)
                    .body(SdkBody::empty())
                    .unwrap(),
            ),
        ]);
        let test_config = Builder::new()
            .region(Region::from_static(REGION))
            .http_client(mock_client.clone())
            .build();
        let s3_client = aws_sdk_s3::Client::from_conf(test_config);
        let store = S3Store::new_with_client_and_jitter(
            &config::stores::S3Store {
                bucket: BUCKET_NAME.to_string(),
                ..Default::default()
            },
            s3_client,
            Arc::new(move |_delay| Duration::from_secs(0)),
        )?;
        let _ = Pin::new(&store).update_oneshot(digest, send_data.clone().into()).await;
        mock_client.assert_requests_match(&[]);
        Ok(())
    }
}
