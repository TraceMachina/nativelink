// Copyright 2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use axum::http::StatusCode;
use base64::Engine as _;
use bytes::Bytes;
use nativelink_config::cas_server::{FetchConfig, FetchDownloaderConfig, WithInstanceName};
use nativelink_config::stores::{MemorySpec, StoreSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::asset::v1::fetch_server::Fetch;
use nativelink_proto::build::bazel::remote::asset::v1::{
    FetchBlobRequest, FetchBlobResponse, FetchDirectoryRequest, Qualifier,
};
use nativelink_proto::build::bazel::remote::execution::v2::Digest;
use nativelink_proto::google::rpc::Status as GoogleStatus;
use nativelink_service::fetch_server::FetchServer;
use nativelink_service::remote_asset_proto::{RemoteAssetArtifact, RemoteAssetQuery};
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::store_trait::{Store, StoreLike};
use tonic::{Request, Status};

async fn make_store_manager() -> Result<Arc<StoreManager>, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        "test_fetch_store",
        store_factory(
            &StoreSpec::Memory(MemorySpec::default()),
            &store_manager,
            None,
        )
        .await?,
    )?;
    Ok(store_manager)
}

#[nativelink_test]
async fn test_fetch_blob() -> Result<(), Status> {
    let store_manager = make_store_manager().await?;
    let instance_name = "foo_instance_name".to_string();
    let fs = FetchServer::new(
        &[WithInstanceName {
            instance_name: instance_name.clone(),
            config: FetchConfig {
                fetch_store: String::from("test_fetch_store"),
                downloader: None,
            },
        }],
        &store_manager,
    )
    .expect("FetchServer config error");

    let remote_asset = RemoteAssetArtifact::new(
        "http://1234".to_owned(),
        vec![Qualifier {
            name: "resource_type".to_owned(),
            value: "test".to_owned(),
        }],
        Digest {
            hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
            size_bytes: 4,
        },
        None,
        0,
    );
    let fetch_store = store_manager.get_store("test_fetch_store").unwrap();
    let digest = remote_asset.digest();
    fetch_store
        .update_oneshot(digest, remote_asset.as_bytes())
        .await
        .unwrap();

    let raw_response: tonic::Response<FetchBlobResponse> = fs
        .fetch_blob(Request::new(FetchBlobRequest {
            instance_name,
            timeout: None,
            oldest_content_accepted: None,
            uris: vec!["http://1234".to_owned()],
            qualifiers: vec![Qualifier {
                name: "resource_type".to_owned(),
                value: "test".to_owned(),
            }],
            digest_function: 0,
        }))
        .await?;
    assert_eq!(
        raw_response.into_inner(),
        FetchBlobResponse {
            status: Some(GoogleStatus {
                code: 0,
                message: "Fetch object found".to_owned(),
                details: vec![]
            }),
            uri: "http://1234".to_owned(),
            qualifiers: vec![Qualifier {
                name: "resource_type".to_owned(),
                value: "test".to_owned(),
            }],
            expires_at: None,
            blob_digest: Some(Digest {
                hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned(),
                size_bytes: 4
            }),
            digest_function: 0
        }
    );
    Ok(())
}

/// A corrupt store entry must return an error, not panic.
#[nativelink_test]
async fn test_fetch_blob_corrupt_store_entry_returns_error() -> Result<(), Status> {
    let store_manager = make_store_manager().await?;
    let instance_name = "foo_instance_name".to_string();
    let fs = FetchServer::new(
        &[WithInstanceName {
            instance_name: instance_name.clone(),
            config: FetchConfig {
                fetch_store: String::from("test_fetch_store"),
                downloader: None,
            },
        }],
        &store_manager,
    )
    .expect("FetchServer config error");

    let query = RemoteAssetQuery::new("http://1234".to_owned(), vec![]);
    let digest = query.digest();
    let fetch_store = store_manager.get_store("test_fetch_store").unwrap();
    fetch_store
        .update_oneshot(digest, Bytes::from_static(&[0xFF, 0xFF, 0xFF]))
        .await
        .unwrap();

    let result = fs
        .fetch_blob(Request::new(FetchBlobRequest {
            instance_name,
            timeout: None,
            oldest_content_accepted: None,
            uris: vec!["http://1234".to_owned()],
            qualifiers: vec![],
            digest_function: 0,
        }))
        .await;
    assert!(
        result.is_err(),
        "corrupt store entry should return an error, not panic: {result:?}"
    );
    Ok(())
}

struct HttpFixture {
    url: String,
    hits: Arc<AtomicUsize>,
    _task: nativelink_util::task::JoinHandleDropGuard<()>,
}

async fn http_fixture(body: Bytes, status: StatusCode, delay: Duration) -> HttpFixture {
    let hits = Arc::new(AtomicUsize::new(0));
    let observed = hits.clone();
    let app = axum::Router::new().fallback(move || {
        let body = body.clone();
        let hits = observed.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            (
                status,
                axum::body::Body::from_stream(futures::stream::iter([Ok::<
                    _,
                    std::convert::Infallible,
                >(body)])),
            )
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = nativelink_util::spawn!("fetch_http_fixture", async move {
        axum::serve(listener, app).await.unwrap();
    });
    HttpFixture {
        url,
        hits,
        _task: task,
    }
}

fn downloader_config() -> FetchDownloaderConfig {
    FetchDownloaderConfig {
        cas_store: "cas".into(),
        http_proxy: None,
        http_proxy_secret_file: None,
        no_proxy: vec![],
        max_size_bytes: 1024 * 1024,
        max_concurrent_downloads: 1,
        timeout_s: 5,
        require_checksum: false,
        allow_unknown_qualifiers: true,
    }
}

async fn downloader_server(
    config: FetchDownloaderConfig,
) -> (FetchServer, Arc<StoreManager>, Arc<MemoryStore>) {
    let stores = make_store_manager().await.unwrap();
    let cas = MemoryStore::new(&MemorySpec::default());
    stores.add_store("cas", Store::new(cas.clone())).unwrap();
    let server = FetchServer::new(
        &[WithInstanceName {
            instance_name: String::new(),
            config: FetchConfig {
                fetch_store: "test_fetch_store".into(),
                downloader: Some(config),
            },
        }],
        &stores,
    )
    .unwrap();
    (server, stores, cas)
}

fn fetch_request(uri: &str) -> FetchBlobRequest {
    FetchBlobRequest {
        uris: vec![uri.into()],
        ..Default::default()
    }
}

async fn fetch(server: &FetchServer, request: FetchBlobRequest) -> FetchBlobResponse {
    server
        .fetch_blob(Request::new(request))
        .await
        .unwrap()
        .into_inner()
}

async fn assert_cas(cas: &MemoryStore, response: &FetchBlobResponse, content: &[u8]) -> DigestInfo {
    assert_eq!(response.status.as_ref().unwrap().code, 0);
    let digest = DigestInfo::try_from(response.blob_digest.as_ref().unwrap()).unwrap();
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(content);
    assert_eq!(digest, hasher.finalize_digest());
    assert_eq!(
        cas.get_part_unchunked(digest, 0, None).await.unwrap(),
        content
    );
    digest
}

#[nativelink_test]
async fn cold_fetch_warm_reuse_and_evicted_cas_refetch() {
    let body = Bytes::from_static(b"shared dependency");
    let origin = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let (server, _stores, cas) = downloader_server(downloader_config()).await;
    let request = fetch_request(&origin.url);
    let first = fetch(&server, request.clone()).await;
    let digest = assert_cas(&cas, &first, &body).await;
    let second = fetch(&server, request.clone()).await;
    assert_cas(&cas, &second, &body).await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
    assert!(cas.remove_entry(digest.into()).await);
    let third = fetch(&server, request).await;
    assert_cas(&cas, &third, &body).await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
}

#[nativelink_test]
async fn explicit_proxy_fetches_an_unresolvable_origin() {
    let body = Bytes::from_static(b"from proxy");
    let proxy = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let mut config = downloader_config();
    config.http_proxy = Some(proxy.url.clone());
    let (server, _stores, cas) = downloader_server(config).await;
    let response = fetch(&server, fetch_request("http://origin.invalid/dependency")).await;
    assert_cas(&cas, &response, &body).await;
    assert_eq!(proxy.hits.load(Ordering::SeqCst), 1);
}

#[nativelink_test]
async fn proxy_secret_file_trims_newlines_and_no_proxy_bypasses_it() {
    let body = Bytes::from_static(b"from proxy file");
    let proxy = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let secret = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(secret.path(), format!("{}\n", proxy.url)).unwrap();
    let mut config = downloader_config();
    config.http_proxy_secret_file = Some(secret.path().to_str().unwrap().into());
    config.no_proxy = vec!["127.0.0.1".into()];
    let (server, _stores, cas) = downloader_server(config).await;
    assert_cas(
        &cas,
        &fetch(&server, fetch_request("http://origin.invalid/proxied")).await,
        &body,
    )
    .await;
    let direct_body = Bytes::from_static(b"direct origin");
    let origin = http_fixture(direct_body.clone(), StatusCode::OK, Duration::ZERO).await;
    assert_cas(
        &cas,
        &fetch(&server, fetch_request(&origin.url)).await,
        &direct_body,
    )
    .await;
    assert!((1..=2).contains(&origin.hits.load(Ordering::SeqCst)));
    assert_eq!(proxy.hits.load(Ordering::SeqCst), 1);
}

#[nativelink_test]
async fn proxy_configuration_is_isolated_between_instances() {
    let a = http_fixture(
        Bytes::from_static(b"instance a"),
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let b = http_fixture(
        Bytes::from_static(b"instance b"),
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let stores = make_store_manager().await.unwrap();
    let cas = MemoryStore::new(&MemorySpec::default());
    stores.add_store("cas", Store::new(cas.clone())).unwrap();
    let configs: Vec<_> = [("a", &a), ("b", &b)]
        .into_iter()
        .map(|(name, proxy)| {
            let mut config = downloader_config();
            config.http_proxy = Some(proxy.url.clone());
            WithInstanceName {
                instance_name: name.into(),
                config: FetchConfig {
                    fetch_store: "test_fetch_store".into(),
                    downloader: Some(config),
                },
            }
        })
        .collect();
    let server = FetchServer::new(&configs, &stores).unwrap();
    // Use different URIs: deliberately sharing mapping/CAS stores also shares their contents.
    for (name, expected) in [("a", b"instance a"), ("b", b"instance b")] {
        let mut request = fetch_request(&format!("http://origin.invalid/{name}"));
        request.instance_name = name.into();
        assert_cas(&cas, &fetch(&server, request).await, expected).await;
    }
    assert_eq!(a.hits.load(Ordering::SeqCst), 1);
    assert_eq!(b.hits.load(Ordering::SeqCst), 1);
}

#[nativelink_test]
async fn lookup_only_never_downloads() {
    let origin = http_fixture(
        Bytes::from_static(b"must not fetch"),
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let stores = make_store_manager().await.unwrap();
    let server = FetchServer::new(
        &[WithInstanceName {
            instance_name: String::new(),
            config: FetchConfig {
                fetch_store: "test_fetch_store".into(),
                downloader: None,
            },
        }],
        &stores,
    )
    .unwrap();
    let response = fetch(&server, fetch_request(&origin.url)).await;
    assert_eq!(response.status.unwrap().code, tonic::Code::NotFound as i32);
    assert_eq!(origin.hits.load(Ordering::SeqCst), 0);
}

#[nativelink_test]
async fn checksum_failure_does_not_publish_and_mirror_can_succeed() {
    let bad = http_fixture(
        Bytes::from_static(b"corrupt dependency"),
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let body = Bytes::from_static(b"correct dependency");
    let good = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let (server, stores, cas) = downloader_server(downloader_config()).await;
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(&body);
    let digest = hasher.finalize_digest();
    let mut request = fetch_request(&bad.url);
    request.qualifiers = vec![Qualifier {
        name: "checksum.sri".into(),
        value: format!(
            "sha256-{}",
            base64::engine::general_purpose::STANDARD.encode(digest.packed_hash().as_slice())
        ),
    }];
    let failed = fetch(&server, request.clone()).await;
    assert_eq!(failed.status.unwrap().code, tonic::Code::DataLoss as i32);
    assert_eq!(cas.len_for_test(), 0);
    assert!(cas.has(digest).await.unwrap().is_none());
    let key = RemoteAssetQuery::new(bad.url.clone(), request.qualifiers.clone()).digest();
    assert!(
        stores
            .get_store("test_fetch_store")
            .unwrap()
            .has(key)
            .await
            .unwrap()
            .is_none()
    );
    request.uris.push(good.url.clone());
    assert_cas(&cas, &fetch(&server, request).await, &body).await;
    assert_eq!(good.hits.load(Ordering::SeqCst), 1);
}

#[nativelink_test]
async fn timeout_and_size_failure_release_the_download_slot() {
    let slow = http_fixture(
        Bytes::from_static(b"slow"),
        StatusCode::OK,
        Duration::from_millis(200),
    )
    .await;
    let large = http_fixture(
        Bytes::from_static(b"too large"),
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let small = http_fixture(Bytes::from_static(b"ok"), StatusCode::OK, Duration::ZERO).await;
    let mut config = downloader_config();
    config.max_size_bytes = 4;
    let (server, _stores, cas) = downloader_server(config).await;
    let mut request = fetch_request(&slow.url);
    request.timeout = Some(prost_types::Duration {
        seconds: 0,
        nanos: 20_000_000,
    });
    let response = fetch(&server, request).await;
    assert_eq!(
        response.status.unwrap().code,
        tonic::Code::DeadlineExceeded as i32
    );
    let response = fetch(&server, fetch_request(&large.url)).await;
    assert_eq!(
        response.status.unwrap().code,
        tonic::Code::ResourceExhausted as i32
    );
    assert_cas(
        &cas,
        &fetch(&server, fetch_request(&small.url)).await,
        b"ok",
    )
    .await;
}

#[nativelink_test]
async fn freshness_reuses_recent_records_and_http_errors_are_not_cached() {
    let origin = http_fixture(Bytes::from_static(b"fresh"), StatusCode::OK, Duration::ZERO).await;
    let error = http_fixture(
        Bytes::from_static(b"error page"),
        StatusCode::BAD_GATEWAY,
        Duration::ZERO,
    )
    .await;
    let (server, stores, cas) = downloader_server(downloader_config()).await;
    let request = fetch_request(&origin.url);
    assert_cas(&cas, &fetch(&server, request.clone()).await, b"fresh").await;
    let index = stores.get_store("test_fetch_store").unwrap();
    let key = RemoteAssetQuery::new(origin.url.clone(), vec![]).digest();
    let record = index.get_part_unchunked(key, 0, None).await.unwrap();
    let artifact = <RemoteAssetArtifact as prost::Message>::decode(record).unwrap();
    let fetched_at = artifact.fetched_at.unwrap();
    let mut constrained = request.clone();
    constrained.oldest_content_accepted = Some(prost_types::Timestamp {
        seconds: 0,
        nanos: 0,
    });
    assert_cas(&cas, &fetch(&server, constrained.clone()).await, b"fresh").await;
    // Equality is fresh enough, including the nanosecond component.
    constrained.oldest_content_accepted = Some(fetched_at);
    assert_cas(&cas, &fetch(&server, constrained.clone()).await, b"fresh").await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
    // Bazel deliberately supplies a future constraint when no checksum is given.
    constrained.oldest_content_accepted =
        Some((SystemTime::now() + Duration::from_hours(1)).into());
    for _ in 0..2 {
        assert_cas(&cas, &fetch(&server, constrained.clone()).await, b"fresh").await;
    }
    assert_eq!(origin.hits.load(Ordering::SeqCst), 3);
    let failed = fetch(&server, fetch_request(&error.url)).await;
    assert_ne!(failed.status.unwrap().code, 0);
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(b"error page");
    assert!(cas.has(hasher.finalize_digest()).await.unwrap().is_none());
}

#[nativelink_test]
async fn unknown_and_duplicate_qualifiers_rejected_before_download() {
    let origin = http_fixture(
        Bytes::from_static(b"unverified"),
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let mut config = downloader_config();
    config.allow_unknown_qualifiers = false;
    let (server, _stores, _cas) = downloader_server(config).await;
    let mut request = fetch_request(&origin.url);
    request.qualifiers = vec![Qualifier {
        name: "unsupported".into(),
        value: "value".into(),
    }];
    assert_eq!(
        server
            .fetch_blob(Request::new(request))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    let checksum = Qualifier {
        name: "checksum.sri".into(),
        value: "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
    };
    let mut request = fetch_request(&origin.url);
    request.qualifiers = vec![checksum.clone(), checksum];
    assert_eq!(
        server
            .fetch_blob(Request::new(request))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(origin.hits.load(Ordering::SeqCst), 0);
}

#[nativelink_test]
async fn invalid_proxy_configuration_returns_error_without_credential_echo() {
    let stores = make_store_manager().await.unwrap();
    for (proxy, file) in [
        (
            Some("http://user:not-real-password@proxy.invalid"),
            Some("missing"),
        ),
        (Some("not a URL with not-real-password"), None),
        (None, Some("missing")),
    ] {
        let mut config = downloader_config();
        config.http_proxy = proxy.map(String::from);
        config.http_proxy_secret_file = file.map(String::from);
        let result = FetchServer::new(
            &[WithInstanceName {
                instance_name: String::new(),
                config: FetchConfig {
                    fetch_store: "test_fetch_store".into(),
                    downloader: Some(config),
                },
            }],
            &stores,
        );
        let error = result.unwrap_err().to_string();
        assert!(!error.contains("not-real-password"));
    }
}

#[nativelink_test]
async fn download_honors_blake3_digest_function_on_cache_reuse() {
    let use_default = std::env::var_os("NATIVELINK_FETCH_TEST_BLAKE3_DEFAULT").is_some();
    if use_default {
        nativelink_util::digest_hasher::set_default_digest_hasher_func(DigestHasherFunc::Blake3)
            .unwrap();
    }
    let body = Bytes::from_static(b"blake3 dependency");
    let origin = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let (server, stores, cas) = downloader_server(downloader_config()).await;
    let mut request = fetch_request(&origin.url);
    request.digest_function = if use_default {
        0
    } else {
        DigestHasherFunc::Blake3.proto_digest_func().into()
    };
    request.qualifiers = vec![sri_with_function(&body, DigestHasherFunc::Blake3)];
    let mut hasher = DigestHasherFunc::Blake3.hasher();
    hasher.update(&body);
    let expected = hasher.finalize_digest();
    for attempt in 0..2 {
        let response = fetch(&server, request.clone()).await;
        assert_eq!(response.status.unwrap().code, 0);
        assert_eq!(
            DigestInfo::try_from(response.blob_digest.unwrap()).unwrap(),
            expected
        );
        assert_eq!(
            DigestHasherFunc::try_from(response.digest_function).unwrap(),
            DigestHasherFunc::Blake3,
        );
        if use_default && attempt == 0 {
            // Zero in a stored record must use the configured default as well.
            let index = stores.get_store("test_fetch_store").unwrap();
            let key =
                RemoteAssetQuery::new(origin.url.clone(), request.qualifiers.clone()).digest();
            let raw = index.get_part_unchunked(key, 0, None).await.unwrap();
            let mut record = <RemoteAssetArtifact as prost::Message>::decode(raw).unwrap();
            record.digest_function = 0;
            index.update_oneshot(key, record.as_bytes()).await.unwrap();
        }
    }
    assert_eq!(
        cas.get_part_unchunked(expected, 0, None).await.unwrap(),
        body
    );
    assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
}

#[nativelink_test]
async fn downloader_config_is_opt_in_and_parses_size_and_duration() {
    let lookup: FetchConfig = serde_json::from_str(r#"{"fetch_store":"mapping"}"#).unwrap();
    assert!(lookup.downloader.is_none());
    let enabled: FetchConfig = serde_json::from_str(
        r#"{
        "fetch_store":"mapping", "downloader": {
            "cas_store":"cas", "http_proxy":"http://proxy.example:3128",
            "no_proxy":["localhost"], "max_size_bytes":"1MiB", "timeout_s":"5s"
        }
    }"#,
    )
    .unwrap();
    let config = enabled.downloader.unwrap();
    assert_eq!(config.max_size_bytes, 1024 * 1024);
    assert_eq!(config.timeout_s, 5);
}

#[nativelink_test]
async fn empty_object_is_valid_but_partial_http_response_is_not_published() {
    let empty = http_fixture(Bytes::new(), StatusCode::OK, Duration::ZERO).await;
    let partial = http_fixture(
        Bytes::from_static(b"partial"),
        StatusCode::PARTIAL_CONTENT,
        Duration::ZERO,
    )
    .await;
    let (server, _stores, cas) = downloader_server(downloader_config()).await;
    assert_cas(&cas, &fetch(&server, fetch_request(&empty.url)).await, b"").await;
    let failed = fetch(&server, fetch_request(&partial.url)).await;
    assert_ne!(failed.status.unwrap().code, 0);
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(b"partial");
    assert!(cas.has(hasher.finalize_digest()).await.unwrap().is_none());
}

#[nativelink_test]
async fn proxy_failure_keeps_the_default_path_available() {
    let origin = http_fixture(
        Bytes::from_static(b"reachable default"),
        StatusCode::OK,
        Duration::from_millis(100),
    )
    .await;
    let proxy = http_fixture(
        Bytes::from_static(b"unavailable"),
        StatusCode::BAD_GATEWAY,
        Duration::ZERO,
    )
    .await;
    let mut config = downloader_config();
    config.http_proxy = Some(proxy.url.clone());
    config.no_proxy = vec!["never.invalid".into()];
    let (server, _stores, cas) = downloader_server(config).await;
    let response = fetch(&server, fetch_request(&origin.url)).await;
    assert_cas(&cas, &response, b"reachable default").await;
    assert_eq!(proxy.hits.load(Ordering::SeqCst), 1);
    assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
}

fn sri(content: &[u8]) -> Qualifier {
    sri_with_function(content, DigestHasherFunc::Sha256)
}

fn sri_with_function(content: &[u8], function: DigestHasherFunc) -> Qualifier {
    let mut hasher = function.hasher();
    hasher.update(content);
    Qualifier {
        name: "checksum.sri".into(),
        value: format!(
            "{}-{}",
            function.to_string().to_lowercase(),
            base64::engine::general_purpose::STANDARD
                .encode(hasher.finalize_digest().packed_hash().as_slice())
        ),
    }
}

#[nativelink_test]
async fn permissive_qualifiers_and_checksum_requirement_are_independent() {
    let body = Bytes::from_static(b"development dependency");
    let origin = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let (server, _stores, cas) = downloader_server(downloader_config()).await;
    let mut request = fetch_request(&origin.url);
    request.qualifiers = vec![Qualifier {
        name: "custom.dependency".into(),
        value: "debug".into(),
    }];
    assert_cas(&cas, &fetch(&server, request.clone()).await, &body).await;
    assert_cas(&cas, &fetch(&server, request.clone()).await, &body).await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
    let mut config = downloader_config();
    config.require_checksum = true;
    let (strict, _stores, strict_cas) = downloader_server(config).await;
    assert_eq!(
        strict
            .fetch_blob(Request::new(request.clone()))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    request.qualifiers.push(sri(&body));
    assert_cas(&strict_cas, &fetch(&strict, request).await, &body).await;
}

#[nativelink_test]
async fn strict_qualifier_validation_applies_before_a_warm_cache_hit() {
    let origin = http_fixture(
        Bytes::from_static(b"unused"),
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let mut config = downloader_config();
    config.allow_unknown_qualifiers = false;
    let (server, stores, cas) = downloader_server(config).await;
    let body = Bytes::from_static(b"cached unknown qualifier");
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(&body);
    let digest = hasher.finalize_digest();
    cas.update_oneshot(digest, body).await.unwrap();
    let mut request = fetch_request(&origin.url);
    request.qualifiers = vec![Qualifier {
        name: "unknown".into(),
        value: "value".into(),
    }];
    let artifact = RemoteAssetArtifact::new(
        origin.url.clone(),
        request.qualifiers.clone(),
        digest.into(),
        None,
        1,
    );
    stores
        .get_store("test_fetch_store")
        .unwrap()
        .update_oneshot(artifact.digest(), artifact.as_bytes())
        .await
        .unwrap();
    assert_eq!(
        server
            .fetch_blob(Request::new(request))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(origin.hits.load(Ordering::SeqCst), 0);
}

#[nativelink_test]
async fn legacy_lookup_preserves_opaque_uris_qualifiers_and_timeouts() {
    let stores = make_store_manager().await.unwrap();
    let server = FetchServer::new(
        &[WithInstanceName {
            instance_name: String::new(),
            config: FetchConfig {
                fetch_store: "test_fetch_store".into(),
                downloader: None,
            },
        }],
        &stores,
    )
    .unwrap();
    let artifact = RemoteAssetArtifact::new(
        "logical:dependency".into(),
        vec![Qualifier {
            name: "custom".into(),
            value: "value".into(),
        }],
        Digest {
            hash: "00".repeat(32),
            size_bytes: 123,
        },
        None,
        1,
    );
    stores
        .get_store("test_fetch_store")
        .unwrap()
        .update_oneshot(artifact.digest(), artifact.as_bytes())
        .await
        .unwrap();
    let response = fetch(
        &server,
        FetchBlobRequest {
            uris: vec![artifact.uri.clone()],
            qualifiers: artifact.qualifiers.clone(),
            timeout: Some(prost_types::Duration {
                seconds: -1,
                nanos: 0,
            }),
            oldest_content_accepted: Some(SystemTime::now().into()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(response.status.unwrap().code, 0);
    assert_eq!(response.blob_digest, artifact.blob_digest);
}

#[nativelink_test]
async fn fast_proxy_wins_without_waiting_for_the_default_path() {
    let body = Bytes::from_static(b"proxy winner");
    let origin = http_fixture(body.clone(), StatusCode::OK, Duration::from_secs(2)).await;
    let proxy = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let mut config = downloader_config();
    config.http_proxy = Some(proxy.url.clone());
    config.no_proxy = vec!["never.invalid".into()];
    let (server, _stores, cas) = downloader_server(config).await;
    let mut request = fetch_request(&origin.url);
    request.qualifiers = vec![sri(&body)];
    request.timeout = Some(prost_types::Duration {
        seconds: 0,
        nanos: 300_000_000,
    });
    assert_cas(&cas, &fetch(&server, request).await, &body).await;
    assert_eq!(proxy.hits.load(Ordering::SeqCst), 1);
}

#[nativelink_test]
async fn different_uris_download_and_register_independently_with_the_same_checksum() {
    let body = Bytes::from_static(b"same immutable dependency");
    let first = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let mirror = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let (server, stores, cas) = downloader_server(downloader_config()).await;
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(&body);
    let digest = hasher.finalize_digest();
    // Bytes preloaded through CAS do not imply that either URI has been registered.
    cas.update_oneshot(digest, body.clone()).await.unwrap();
    for origin in [&first, &mirror] {
        let mut request = fetch_request(&origin.url);
        request.qualifiers = vec![sri(&body)];
        assert_cas(&cas, &fetch(&server, request.clone()).await, &body).await;
        assert_cas(&cas, &fetch(&server, request.clone()).await, &body).await;
        assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
        let key = RemoteAssetQuery::new(origin.url.clone(), request.qualifiers).digest();
        let record = stores
            .get_store("test_fetch_store")
            .unwrap()
            .get_part_unchunked(key, 0, None)
            .await
            .unwrap();
        let artifact = <RemoteAssetArtifact as prost::Message>::decode(record).unwrap();
        assert_eq!(artifact.uri, origin.url);
        assert_eq!(
            DigestInfo::try_from(artifact.blob_digest.unwrap()).unwrap(),
            digest
        );
    }
}

// Child test processes isolate proxy environment changes from concurrent tests.
#[test]
fn proxy_configuration_is_independent_of_inherited_environment() {
    for (no_proxy, test_name) in [
        ("", "cold_fetch_warm_reuse_and_evicted_cas_refetch"),
        ("*", "explicit_proxy_fetches_an_unresolvable_origin"),
    ] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name])
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("http_proxy", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("https_proxy", "http://127.0.0.1:9")
            .env("ALL_PROXY", "http://127.0.0.1:9")
            .env("all_proxy", "http://127.0.0.1:9")
            .env("NO_PROXY", no_proxy)
            .env("no_proxy", no_proxy)
            .output()
            .unwrap();
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
            "{test_name}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[nativelink_test]
async fn freshness_migrates_records_without_timestamps_only_when_required() {
    let origin = http_fixture(
        Bytes::from_static(b"legacy"),
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let (server, stores, cas) = downloader_server(downloader_config()).await;
    let request = fetch_request(&origin.url);
    let digest = assert_cas(&cas, &fetch(&server, request.clone()).await, b"legacy").await;
    let index = stores.get_store("test_fetch_store").unwrap();
    // The old record format has no tag 6 and decodes with fetched_at = None.
    let legacy = RemoteAssetArtifact::new(origin.url.clone(), vec![], digest.into(), None, 1);
    let key = legacy.digest();
    index.update_oneshot(key, legacy.as_bytes()).await.unwrap();
    assert_cas(&cas, &fetch(&server, request.clone()).await, b"legacy").await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
    let mut constrained = request;
    constrained.oldest_content_accepted = Some(prost_types::Timestamp {
        seconds: 0,
        nanos: 0,
    });
    assert_cas(&cas, &fetch(&server, constrained.clone()).await, b"legacy").await;
    let record = index.get_part_unchunked(key, 0, None).await.unwrap();
    let updated = <RemoteAssetArtifact as prost::Message>::decode(record).unwrap();
    assert!(updated.fetched_at.is_some());
    assert_eq!(updated.digest(), key);
    assert_cas(&cas, &fetch(&server, constrained).await, b"legacy").await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
}

#[nativelink_test]
async fn freshness_uses_retrieval_start_instead_of_completion() {
    let origin = http_fixture(
        Bytes::from_static(b"slow"),
        StatusCode::OK,
        Duration::from_millis(100),
    )
    .await;
    let (server, _stores, cas) = downloader_server(downloader_config()).await;
    let request = fetch_request(&origin.url);
    let (first, during_retrieval) = tokio::join!(fetch(&server, request.clone()), async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while origin.hits.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        SystemTime::now().into()
    },);
    assert_cas(&cas, &first, b"slow").await;
    let mut constrained = request;
    constrained.oldest_content_accepted = Some(during_retrieval);
    assert_cas(&cas, &fetch(&server, constrained).await, b"slow").await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
}

#[nativelink_test]
async fn default_size_limit_accepts_an_object_larger_than_256_mib() {
    // Stream a large dependency without allocating a large response body at origin.
    let chunk = Bytes::from(vec![42; 1024 * 1024]);
    let size = 256 * chunk.len() as u64 + 1;
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    for _ in 0..256 {
        hasher.update(&chunk);
    }
    hasher.update(b"!");
    let expected = hasher.finalize_digest();
    let app = axum::Router::new().fallback(move || {
        let chunk = chunk.clone();
        async move {
            let chunks = (0..256)
                .map(move |_| Ok::<_, std::convert::Infallible>(chunk.clone()))
                .chain(std::iter::once(Ok(Bytes::from_static(b"!"))));
            (
                [(axum::http::header::CONTENT_LENGTH, size.to_string())],
                axum::body::Body::from_stream(futures::stream::iter(chunks)),
            )
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let _task = nativelink_util::spawn!("fetch_large_origin", async move {
        axum::serve(listener, app).await.unwrap();
    });
    // An omitted limit and explicit zero both select unlimited downloads.
    let config: FetchDownloaderConfig =
        serde_json::from_str(r#"{"cas_store":"cas","timeout_s":"120s"}"#).unwrap();
    let explicit_zero: FetchDownloaderConfig =
        serde_json::from_str(r#"{"cas_store":"cas","max_size_bytes":0}"#).unwrap();
    assert_eq!(config.max_size_bytes, explicit_zero.max_size_bytes);
    let (server, _stores, cas) = downloader_server(config).await;
    let response = fetch(&server, fetch_request(&url)).await;
    assert_eq!(response.status.as_ref().unwrap().code, 0);
    assert_eq!(
        DigestInfo::try_from(response.blob_digest.as_ref().unwrap()).unwrap(),
        expected
    );
    assert_eq!(cas.has(expected).await.unwrap(), Some(size));
    assert_eq!(
        cas.get_part_unchunked(expected, size - 1, Some(1))
            .await
            .unwrap(),
        b"!".as_slice()
    );
}

#[nativelink_test]
async fn checksum_algorithm_can_match_or_differ_from_cas_digest() {
    for checksum_function in [DigestHasherFunc::Sha256, DigestHasherFunc::Blake3] {
        for cas_function in [DigestHasherFunc::Sha256, DigestHasherFunc::Blake3] {
            let body = Bytes::from_static(b"algorithm-independent dependency");
            let origin = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
            let (server, _stores, cas) = downloader_server(downloader_config()).await;
            let mut hasher = cas_function.hasher();
            hasher.update(&body);
            let expected = hasher.finalize_digest();
            let mut request = fetch_request(&origin.url);
            request.digest_function = cas_function.proto_digest_func().into();
            request.qualifiers = vec![sri_with_function(b"incorrect bytes", checksum_function)];
            let rejected = fetch(&server, request.clone()).await;
            assert_eq!(rejected.status.unwrap().code, tonic::Code::DataLoss as i32);
            assert!(cas.has(expected).await.unwrap().is_none());
            request.qualifiers = vec![sri_with_function(&body, checksum_function)];
            for _ in 0..2 {
                let response = fetch(&server, request.clone()).await;
                assert_eq!(response.status.unwrap().code, 0);
                assert_eq!(response.digest_function, request.digest_function);
                assert_eq!(
                    DigestInfo::try_from(response.blob_digest.unwrap()).unwrap(),
                    expected
                );
            }
            assert_eq!(
                cas.get_part_unchunked(expected, 0, None).await.unwrap(),
                body
            );
            assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
        }
    }
}

#[nativelink_test]
async fn unsupported_or_malformed_checksums_fail_before_origin_access() {
    let origin = http_fixture(
        Bytes::from_static(b"unused"),
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let (server, _stores, _cas) = downloader_server(downloader_config()).await;
    for value in [
        "",
        " \t",
        "sha512-AAAA",
        "blake3-@@@",
        "blake3-AA==",
        "blake3",
        "sha256-@@@ sha512-AAAA",
    ] {
        let mut request = fetch_request(&origin.url);
        request.qualifiers = vec![Qualifier {
            name: "checksum.sri".into(),
            value: value.into(),
        }];
        assert_eq!(
            server
                .fetch_blob(Request::new(request))
                .await
                .unwrap_err()
                .code(),
            tonic::Code::InvalidArgument
        );
    }
    assert_eq!(origin.hits.load(Ordering::SeqCst), 0);
}

#[test]
fn configured_default_digest_is_not_forced_to_sha256() {
    // A separate process isolates the default's OnceLock from all other tests.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "download_honors_blake3_digest_function_on_cache_reuse",
        ])
        .env("NATIVELINK_FETCH_TEST_BLAKE3_DEFAULT", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[nativelink_test]
async fn fetch_logs_do_not_include_uri_or_qualifier_credentials() {
    let markers = [
        "fetch-user-marker",
        "fetch-password-marker",
        "fetch-query-marker",
        "fetch-fragment-marker",
        "fetch-auth-marker",
        "fetch-cookie-marker",
        "fetch-custom-marker",
    ];
    let origin = http_fixture(
        Bytes::from_static(b"private dependency"),
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let uri = origin.url.replacen(
        "http://",
        "http://fetch-user-marker:fetch-password-marker@",
        1,
    ) + "/artifact?token=fetch-query-marker#fetch-fragment-marker";
    let (server, stores, _cas) = downloader_server(downloader_config()).await;
    let mut request = fetch_request(&uri);
    request.qualifiers = vec![
        Qualifier {
            name: "http_header:Authorization".into(),
            value: "Bearer fetch-auth-marker".into(),
        },
        Qualifier {
            name: "http_header_url:0:Cookie".into(),
            value: "session=fetch-cookie-marker".into(),
        },
        Qualifier {
            name: "custom".into(),
            value: "fetch-custom-marker".into(),
        },
    ];
    // Both the cold result and warm index result echo metadata to the caller,
    // but neither the request nor response may be dumped into tracing.
    for _ in 0..2 {
        let response = fetch(&server, request.clone()).await;
        assert_eq!(response.status.unwrap().code, 0);
        assert_eq!(response.uri, uri);
        assert_eq!(response.qualifiers, request.qualifiers);
    }
    assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
    let lookup_only = FetchServer::new(
        &[WithInstanceName {
            instance_name: String::new(),
            config: FetchConfig {
                fetch_store: "test_fetch_store".into(),
                downloader: None,
            },
        }],
        &stores,
    )
    .unwrap();
    let response = fetch(&lookup_only, request.clone()).await;
    assert_eq!(response.qualifiers, request.qualifiers);
    assert_eq!(response.uri, uri);
    let directory = server
        .fetch_directory(Request::new(FetchDirectoryRequest {
            uris: request.uris,
            qualifiers: request.qualifiers,
            ..Default::default()
        }))
        .await;
    assert_eq!(directory.unwrap_err().code(), tonic::Code::Unimplemented);
    logs_assert(|lines| {
        if !lines
            .iter()
            .any(|line| line.contains("Looked up fetch asset"))
        {
            return Err("Fetch tracing was not captured".into());
        }
        for marker in markers {
            if lines.iter().any(|line| line.contains(marker)) {
                return Err(format!(
                    "Fetch credential marker appeared in logs: {marker}"
                ));
            }
        }
        Ok(())
    });
}

#[nativelink_test]
async fn cache_key_collision_does_not_bypass_requested_checksum() {
    let body = Bytes::from_static(b"content that does not match the requested checksum");
    let origin = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let (server, _stores, cas) = downloader_server(downloader_config()).await;
    let qualifier = sri(b"expected content");
    let uri = format!("{}/", origin.url);
    let colliding_uri = format!("{}{}{}", uri, qualifier.name, qualifier.value);
    assert_eq!(
        RemoteAssetQuery::new(colliding_uri.clone(), vec![]).digest(),
        RemoteAssetQuery::new(uri.clone(), vec![qualifier.clone()]).digest(),
    );
    let first = fetch(&server, fetch_request(&colliding_uri)).await;
    assert_cas(&cas, &first, &body).await;
    let mut request = fetch_request(&uri);
    request.qualifiers = vec![qualifier];
    let response = fetch(&server, request).await;
    assert_eq!(response.status.unwrap().code, tonic::Code::DataLoss as i32);
    assert!(response.blob_digest.is_none());
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
    // The original identity can still reuse its record without a checksum.
    assert_cas(
        &cas,
        &fetch(&server, fetch_request(&colliding_uri)).await,
        &body,
    )
    .await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
}

#[nativelink_test]
async fn cache_key_collision_checks_qualifier_boundaries() {
    let body = Bytes::from_static(b"opaque qualifiers");
    let origin = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let (server, _stores, cas) = downloader_server(downloader_config()).await;
    let mut first = fetch_request(&origin.url);
    first.qualifiers = vec![Qualifier {
        name: "a".into(),
        value: "bc".into(),
    }];
    let mut second = fetch_request(&origin.url);
    second.qualifiers = vec![Qualifier {
        name: "ab".into(),
        value: "c".into(),
    }];
    assert_eq!(
        RemoteAssetQuery::new(origin.url.clone(), first.qualifiers.clone()).digest(),
        RemoteAssetQuery::new(origin.url.clone(), second.qualifiers.clone()).digest(),
    );
    assert_cas(&cas, &fetch(&server, first).await, &body).await;
    let response = fetch(&server, second.clone()).await;
    assert_eq!(response.qualifiers, second.qualifiers);
    assert_cas(&cas, &response, &body).await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
    assert_cas(&cas, &fetch(&server, second).await, &body).await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
}

#[nativelink_test]
async fn checksum_sri_accepts_supported_alternatives() {
    for cas_function in [DigestHasherFunc::Sha256, DigestHasherFunc::Blake3] {
        for matching_function in [DigestHasherFunc::Sha256, DigestHasherFunc::Blake3] {
            let body = Bytes::from_static(b"checksum alternatives");
            let origin = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
            let (server, _stores, cas) = downloader_server(downloader_config()).await;
            let mut request = fetch_request(&origin.url);
            request.digest_function = cas_function.proto_digest_func().into();
            request.qualifiers = vec![Qualifier {
                name: "checksum.sri".into(),
                value: format!(
                    "  {}\t{}\n{}  ",
                    sri_with_function(b"wrong sha256", DigestHasherFunc::Sha256).value,
                    sri_with_function(b"wrong blake3", DigestHasherFunc::Blake3).value,
                    sri_with_function(&body, matching_function).value,
                ),
            }];
            let mut hasher = cas_function.hasher();
            hasher.update(&body);
            let expected = hasher.finalize_digest();
            for _ in 0..2 {
                let response = fetch(&server, request.clone()).await;
                assert_eq!(response.status.unwrap().code, 0);
                assert_eq!(response.digest_function, request.digest_function);
                assert_eq!(
                    DigestInfo::try_from(response.blob_digest.unwrap()).unwrap(),
                    expected
                );
            }
            assert_eq!(
                cas.get_part_unchunked(expected, 0, None).await.unwrap(),
                body
            );
            assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
        }
    }
}

#[nativelink_test]
async fn checksum_alternatives_must_match_before_publication() {
    let body = Bytes::from_static(b"must remain unpublished");
    let origin = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let (server, stores, cas) = downloader_server(downloader_config()).await;
    let mut request = fetch_request(&origin.url);
    request.qualifiers = vec![Qualifier {
        name: "checksum.sri".into(),
        value: format!("{} {}", sri(b"wrong one").value, sri(b"wrong two").value),
    }];
    let key = RemoteAssetQuery::new(origin.url.clone(), request.qualifiers.clone()).digest();
    let response = fetch(&server, request).await;
    assert_eq!(response.status.unwrap().code, tonic::Code::DataLoss as i32);
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(&body);
    assert!(cas.has(hasher.finalize_digest()).await.unwrap().is_none());
    assert!(
        stores
            .get_store("test_fetch_store")
            .unwrap()
            .has(key)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
}

#[nativelink_test]
async fn unsupported_checksum_alternative_does_not_hide_supported_match() {
    let body = Bytes::from_static(b"supported match");
    let origin = http_fixture(body.clone(), StatusCode::OK, Duration::ZERO).await;
    let (server, _stores, cas) = downloader_server(downloader_config()).await;
    let mut request = fetch_request(&origin.url);
    let unsupported = format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode([0; 64])
    );
    request.qualifiers = vec![Qualifier {
        name: "checksum.sri".into(),
        value: format!("{} {}", unsupported, sri(&body).value),
    }];
    assert_cas(&cas, &fetch(&server, request).await, &body).await;
    assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
}
