// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::pin::Pin;
use core::sync::atomic::Ordering;
use core::time::Duration;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use nativelink_config::cas_server::WithInstanceName;
use nativelink_config::stores::{MemorySpec, StoreSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_server::ContentAddressableStorage;
use nativelink_proto::build::bazel::remote::execution::v2::{
    BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, Digest, Directory, DirectoryNode, FindMissingBlobsRequest,
    GetTreeRequest, GetTreeResponse, NodeProperties, SpliceBlobRequest, SplitBlobRequest,
    SplitBlobResponse, batch_read_blobs_response, batch_update_blobs_request,
    batch_update_blobs_response, chunking_function, compressor, digest_function,
};
use nativelink_proto::google::rpc::Status as GrpcStatus;
use nativelink_service::cas_server::CasServer;
use nativelink_store::ac_utils::serialize_and_upload_message;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use prost::Message;
use prost_types::Timestamp;
use tonic::{Code, Request};

const INSTANCE_NAME: &str = "foo_instance_name";
const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";
const HASH2: &str = "9993456789abcdef000000000000000000000000000000000123456789abc999";
const HASH3: &str = "7773456789abcdef000000000000000000000000000000000123456789abc777";
const BAD_HASH: &str = "BAD_HASH";

async fn make_store_manager() -> Result<Arc<StoreManager>, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        "main_cas",
        store_factory(
            &StoreSpec::Memory(MemorySpec::default()),
            &store_manager,
            None,
        )
        .await?,
    );
    Ok(store_manager)
}

fn make_cas_server(store_manager: &StoreManager) -> Result<CasServer, Error> {
    CasServer::new(
        &[WithInstanceName {
            instance_name: "foo_instance_name".to_string(),
            config: nativelink_config::cas_server::CasStoreConfig {
                cas_store: "main_cas".to_string(),
                experimental_chunking: None,
            },
        }],
        store_manager,
    )
}

#[nativelink_test]
async fn empty_store() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;

    let raw_response = cas_server
        .find_missing_blobs(Request::new(FindMissingBlobsRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digests: vec![Digest {
                hash: HASH1.to_string(),
                size_bytes: 0,
            }],
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await;
    assert!(raw_response.is_ok());
    let response = raw_response.unwrap().into_inner();
    assert_eq!(response.missing_blob_digests.len(), 1);
    Ok(())
}

#[nativelink_test]
async fn store_one_item_existence() -> Result<(), Box<dyn core::error::Error>> {
    const VALUE: &str = "1";

    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    store
        .update_oneshot(DigestInfo::try_new(HASH1, VALUE.len())?, VALUE.into())
        .await?;
    let raw_response = cas_server
        .find_missing_blobs(Request::new(FindMissingBlobsRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digests: vec![Digest {
                hash: HASH1.to_string(),
                size_bytes: VALUE.len() as i64,
            }],
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await;
    assert!(raw_response.is_ok());
    let response = raw_response.unwrap().into_inner();
    assert_eq!(response.missing_blob_digests.len(), 0); // All items should have been found.
    Ok(())
}

#[nativelink_test]
async fn has_three_requests_one_bad_hash() -> Result<(), Box<dyn core::error::Error>> {
    const VALUE: &str = "1";

    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    store
        .update_oneshot(DigestInfo::try_new(HASH1, VALUE.len())?, VALUE.into())
        .await?;
    let raw_response = cas_server
        .find_missing_blobs(Request::new(FindMissingBlobsRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digests: vec![
                Digest {
                    hash: HASH1.to_string(),
                    size_bytes: VALUE.len() as i64,
                },
                Digest {
                    hash: BAD_HASH.to_string(),
                    size_bytes: VALUE.len() as i64,
                },
                Digest {
                    hash: HASH1.to_string(),
                    size_bytes: VALUE.len() as i64,
                },
            ],
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await;
    let error = raw_response.unwrap_err();
    assert!(
        error.to_string().contains("Invalid sha256 hash: BAD_HASH"),
        "'Invalid sha256 hash: BAD_HASH' not found in: {error:?}"
    );
    Ok(())
}

#[nativelink_test]
async fn update_existing_item() -> Result<(), Box<dyn core::error::Error>> {
    const VALUE1: &str = "1";
    const VALUE2: &str = "2";

    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    let digest = Digest {
        hash: HASH1.to_string(),
        size_bytes: VALUE2.len() as i64,
    };

    store
        .update_oneshot(DigestInfo::try_new(HASH1, VALUE1.len())?, VALUE1.into())
        .await
        .expect("Update should have succeeded");

    let raw_response = cas_server
        .batch_update_blobs(Request::new(BatchUpdateBlobsRequest {
            instance_name: INSTANCE_NAME.to_string(),
            requests: vec![batch_update_blobs_request::Request {
                digest: Some(digest.clone()),
                data: VALUE2.into(),
                compressor: compressor::Value::Identity.into(),
            }],
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await;
    assert!(raw_response.is_ok());
    assert_eq!(
        raw_response.unwrap().into_inner(),
        BatchUpdateBlobsResponse {
            responses: vec![batch_update_blobs_response::Response {
                digest: Some(digest),
                status: Some(GrpcStatus {
                    code: 0, // Status Ok.
                    message: String::new(),
                    details: vec![],
                }),
            },],
        }
    );
    let new_data = store
        .get_part_unchunked(DigestInfo::try_new(HASH1, VALUE1.len())?, 0, None)
        .await
        .expect("Get should have succeeded");
    assert_eq!(
        new_data,
        VALUE2.as_bytes(),
        "Expected store to have been updated to new value"
    );
    Ok(())
}

#[nativelink_test]
async fn batch_read_blobs_read_two_blobs_success_one_fail()
-> Result<(), Box<dyn core::error::Error>> {
    const VALUE1: &str = "1";
    const VALUE2: &str = "23";

    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    let digest1 = Digest {
        hash: HASH1.to_string(),
        size_bytes: VALUE1.len() as i64,
    };
    let digest2 = Digest {
        hash: HASH2.to_string(),
        size_bytes: VALUE2.len() as i64,
    };
    {
        // Insert dummy data.
        store
            .update_oneshot(DigestInfo::try_new(HASH1, VALUE1.len())?, VALUE1.into())
            .await
            .expect("Update should have succeeded");
        store
            .update_oneshot(DigestInfo::try_new(HASH2, VALUE2.len())?, VALUE2.into())
            .await
            .expect("Update should have succeeded");
    }
    {
        // Read two blobs and additional blob should come back not found.
        let digest3 = Digest {
            hash: HASH3.to_string(),
            size_bytes: 3,
        };
        let raw_response = cas_server
            .batch_read_blobs(Request::new(BatchReadBlobsRequest {
                instance_name: INSTANCE_NAME.to_string(),
                digests: vec![digest1.clone(), digest2.clone(), digest3.clone()],
                acceptable_compressors: vec![compressor::Value::Identity.into()],
                digest_function: digest_function::Value::Sha256.into(),
            }))
            .await;
        assert!(raw_response.is_ok());
        assert_eq!(
            raw_response.unwrap().into_inner(),
            BatchReadBlobsResponse {
                responses: vec![
                    batch_read_blobs_response::Response {
                        digest: Some(digest1),
                        data: VALUE1.into(),
                        status: Some(GrpcStatus {
                            code: 0, // Status Ok.
                            message: String::new(),
                            details: vec![],
                        }),
                        compressor: compressor::Value::Identity.into(),
                    },
                    batch_read_blobs_response::Response {
                        digest: Some(digest2),
                        data: VALUE2.into(),
                        status: Some(GrpcStatus {
                            code: 0, // Status Ok.
                            message: String::new(),
                            details: vec![],
                        }),
                        compressor: compressor::Value::Identity.into(),
                    },
                    batch_read_blobs_response::Response {
                        digest: Some(digest3.clone()),
                        data: vec![].into(),
                        status: Some(GrpcStatus {
                            code: Code::NotFound as i32,
                            message: format!(
                                "Key {:?} not found",
                                StoreKey::from(DigestInfo::try_from(digest3)?)
                            ),
                            details: vec![],
                        }),
                        compressor: compressor::Value::Identity.into(),
                    }
                ],
            }
        );
    }
    Ok(())
}

struct SetupDirectoryResult {
    root_directory: Directory,
    root_directory_digest_info: DigestInfo,
    sub_directories: Vec<Directory>,
    sub_directory_digest_infos: Vec<DigestInfo>,
}
async fn setup_directory_structure(
    store_pinned: Pin<&impl StoreLike>,
) -> Result<SetupDirectoryResult, Error> {
    // Set up 5 sub-directories.
    const SUB_DIRECTORIES_LENGTH: i32 = 5;
    let mut sub_directory_nodes: Vec<DirectoryNode> = vec![];
    let mut sub_directories: Vec<Directory> = vec![];
    let mut sub_directory_digest_infos: Vec<DigestInfo> = vec![];

    for i in 0..SUB_DIRECTORIES_LENGTH {
        let sub_directory: Directory = Directory {
            files: vec![],
            directories: vec![],
            symlinks: vec![],
            node_properties: Some(NodeProperties {
                properties: vec![],
                mtime: Some(Timestamp {
                    seconds: i64::from(i),
                    nanos: 0,
                }),
                unix_mode: Some(0o755),
            }),
        };
        let sub_directory_digest_info: DigestInfo = serialize_and_upload_message(
            &sub_directory,
            store_pinned,
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        sub_directory_digest_infos.push(sub_directory_digest_info);
        sub_directory_nodes.push(DirectoryNode {
            name: format!("sub_directory_{i}"),
            digest: Some(sub_directory_digest_info.into()),
        });
        sub_directories.push(sub_directory);
    }

    // Set up a root directory.
    let root_directory: Directory = Directory {
        files: vec![],
        directories: sub_directory_nodes,
        symlinks: vec![],
        node_properties: None,
    };
    let root_directory_digest_info: DigestInfo = serialize_and_upload_message(
        &root_directory,
        store_pinned,
        &mut DigestHasherFunc::Sha256.hasher(),
    )
    .await?;

    Ok(SetupDirectoryResult {
        root_directory,
        root_directory_digest_info,
        sub_directories,
        sub_directory_digest_infos,
    })
}

#[nativelink_test]
async fn get_tree_read_directories_without_paging() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    // Setup directory structure.
    let SetupDirectoryResult {
        root_directory,
        root_directory_digest_info,
        sub_directories,
        sub_directory_digest_infos: _,
    } = setup_directory_structure(store.as_pin()).await?;

    // Must work when paging is disabled ( `page_size` is 0 ).
    // It reads all directories at once.

    // First verify that using an empty page token is treated as if the client had sent the root
    // digest.
    {
        let raw_response = cas_server
            .get_tree(Request::new(GetTreeRequest {
                instance_name: INSTANCE_NAME.to_string(),
                page_size: 0,
                page_token: String::new(),
                root_digest: Some(root_directory_digest_info.into()),
                digest_function: digest_function::Value::Sha256.into(),
            }))
            .await;
        assert_eq!(
            raw_response
                .unwrap()
                .into_inner()
                .filter_map(|x| async move { Some(x.unwrap()) })
                .collect::<Vec<_>>()
                .await,
            vec![GetTreeResponse {
                directories: vec![
                    root_directory.clone(),
                    sub_directories[0].clone(),
                    sub_directories[1].clone(),
                    sub_directories[2].clone(),
                    sub_directories[3].clone(),
                    sub_directories[4].clone()
                ],
                next_page_token: String::new()
            }]
        );
    }

    // Also verify that sending the root digest returns the entire tree as well.
    {
        let raw_response = cas_server
            .get_tree(Request::new(GetTreeRequest {
                instance_name: INSTANCE_NAME.to_string(),
                page_size: 0,
                page_token: format!("{root_directory_digest_info}"),
                root_digest: Some(root_directory_digest_info.into()),
                digest_function: digest_function::Value::Sha256.into(),
            }))
            .await;
        assert_eq!(
            raw_response
                .unwrap()
                .into_inner()
                .filter_map(|x| async move { Some(x.unwrap()) })
                .collect::<Vec<_>>()
                .await,
            vec![GetTreeResponse {
                directories: vec![
                    root_directory.clone(),
                    sub_directories[0].clone(),
                    sub_directories[1].clone(),
                    sub_directories[2].clone(),
                    sub_directories[3].clone(),
                    sub_directories[4].clone()
                ],
                next_page_token: String::new()
            }]
        );
    }

    Ok(())
}

#[nativelink_test]
async fn get_tree_read_directories_with_paging() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    // Setup directory structure.
    let SetupDirectoryResult {
        root_directory,
        root_directory_digest_info,
        sub_directories,
        sub_directory_digest_infos,
    } = setup_directory_structure(store.as_pin()).await?;

    // Must work when paging is enabled ( `page_size` is 2 ).
    // First, it reads `root_directory` and `sub_directory[0]`.
    // Then, it reads `sub_directory[1]` and `sub_directory[2]`.
    // Finally, it reads `sub_directory[3]` and `sub_directory[4]`.

    // First, verify that an empty initial page token is treated as if the client had sent the
    // root digest and respects the page size.
    {
        let raw_response = cas_server
            .get_tree(Request::new(GetTreeRequest {
                instance_name: INSTANCE_NAME.to_string(),
                page_size: 2,
                page_token: String::new(),
                root_digest: Some(root_directory_digest_info.into()),
                digest_function: digest_function::Value::Sha256.into(),
            }))
            .await;
        assert_eq!(
            raw_response
                .unwrap()
                .into_inner()
                .filter_map(|x| async move { Some(x.unwrap()) })
                .collect::<Vec<_>>()
                .await,
            vec![GetTreeResponse {
                directories: vec![root_directory.clone(), sub_directories[0].clone()],
                next_page_token: format!("{}", sub_directory_digest_infos[1]),
            }]
        );
    }

    // Also verify that sending the root digest as the page token is treated as paging from the
    // beginning and respects page size.
    {
        let raw_response = cas_server
            .get_tree(Request::new(GetTreeRequest {
                instance_name: INSTANCE_NAME.to_string(),
                page_size: 2,
                page_token: format!("{root_directory_digest_info}"),
                root_digest: Some(root_directory_digest_info.into()),
                digest_function: digest_function::Value::Sha256.into(),
            }))
            .await;
        assert_eq!(
            raw_response
                .unwrap()
                .into_inner()
                .filter_map(|x| async move { Some(x.unwrap()) })
                .collect::<Vec<_>>()
                .await,
            vec![GetTreeResponse {
                directories: vec![root_directory.clone(), sub_directories[0].clone()],
                next_page_token: format!("{}", sub_directory_digest_infos[1]),
            }]
        );
    }

    // Verify that paging from a non-initial page token will return the expected content.
    {
        let raw_response = cas_server
            .get_tree(Request::new(GetTreeRequest {
                instance_name: INSTANCE_NAME.to_string(),
                page_size: 2,
                page_token: format!("{}", sub_directory_digest_infos[1]),
                root_digest: Some(root_directory_digest_info.into()),
                digest_function: digest_function::Value::Sha256.into(),
            }))
            .await;
        assert_eq!(
            raw_response
                .unwrap()
                .into_inner()
                .filter_map(|x| async move { Some(x.unwrap()) })
                .collect::<Vec<_>>()
                .await,
            vec![GetTreeResponse {
                directories: vec![sub_directories[1].clone(), sub_directories[2].clone()],
                next_page_token: format!("{}", sub_directory_digest_infos[3]),
            }]
        );

        let raw_response = cas_server
            .get_tree(Request::new(GetTreeRequest {
                instance_name: INSTANCE_NAME.to_string(),
                page_size: 2,
                page_token: format!("{}", sub_directory_digest_infos[3]),
                root_digest: Some(root_directory_digest_info.into()),
                digest_function: digest_function::Value::Sha256.into(),
            }))
            .await;
        assert_eq!(
            raw_response
                .unwrap()
                .into_inner()
                .filter_map(|x| async move { Some(x.unwrap()) })
                .collect::<Vec<_>>()
                .await,
            vec![GetTreeResponse {
                directories: vec![sub_directories[3].clone(), sub_directories[4].clone()],
                next_page_token: String::new(),
            }]
        );
    }

    Ok(())
}

#[nativelink_test]
async fn batch_update_blobs_two_items_existence_with_third_missing()
-> Result<(), Box<dyn core::error::Error>> {
    const VALUE1: &str = "1";
    const VALUE2: &str = "23";

    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;

    let digest1 = Digest {
        hash: HASH1.to_string(),
        size_bytes: VALUE1.len() as i64,
    };
    let digest2 = Digest {
        hash: HASH2.to_string(),
        size_bytes: VALUE2.len() as i64,
    };

    {
        // Send update to insert two entries into backend.
        let raw_response = cas_server
            .batch_update_blobs(Request::new(BatchUpdateBlobsRequest {
                instance_name: INSTANCE_NAME.to_string(),
                requests: vec![
                    batch_update_blobs_request::Request {
                        digest: Some(digest1.clone()),
                        data: VALUE1.into(),
                        compressor: compressor::Value::Identity.into(),
                    },
                    batch_update_blobs_request::Request {
                        digest: Some(digest2.clone()),
                        data: VALUE2.into(),
                        compressor: compressor::Value::Identity.into(),
                    },
                ],
                digest_function: digest_function::Value::Sha256.into(),
            }))
            .await;
        assert!(raw_response.is_ok());
        assert_eq!(
            raw_response.unwrap().into_inner(),
            BatchUpdateBlobsResponse {
                responses: vec![
                    batch_update_blobs_response::Response {
                        digest: Some(digest1),
                        status: Some(GrpcStatus {
                            code: 0, // Status Ok.
                            message: String::new(),
                            details: vec![],
                        }),
                    },
                    batch_update_blobs_response::Response {
                        digest: Some(digest2),
                        status: Some(GrpcStatus {
                            code: 0, // Status Ok.
                            message: String::new(),
                            details: vec![],
                        }),
                    }
                ],
            }
        );
    }
    {
        // Query the backend for inserted entries plus one that is not
        // present and ensure it only returns the one that is missing.
        let missing_digest = Digest {
            hash: HASH3.to_string(),
            size_bytes: 1,
        };
        let raw_response = cas_server
            .find_missing_blobs(Request::new(FindMissingBlobsRequest {
                instance_name: INSTANCE_NAME.to_string(),
                blob_digests: vec![
                    Digest {
                        hash: HASH1.to_string(),
                        size_bytes: VALUE1.len() as i64,
                    },
                    missing_digest.clone(),
                    Digest {
                        hash: HASH2.to_string(),
                        size_bytes: VALUE2.len() as i64,
                    },
                ],
                digest_function: digest_function::Value::Sha256.into(),
            }))
            .await;
        assert!(raw_response.is_ok());
        let response = raw_response.unwrap().into_inner();
        assert_eq!(response.missing_blob_digests, vec![missing_digest]);
    }
    Ok(())
}

#[derive(Debug, MetricsComponent)]
struct StallStore {
    delay: Duration,
}

#[async_trait]
impl StoreDriver for StallStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        _digests: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        for r in results.iter_mut() {
            *r = None;
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        tokio::time::sleep(self.delay).await;
        Ok(0)
    }

    async fn get_part(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        tokio::time::sleep(self.delay).await;
        Ok(())
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &'_ dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(
        self: Arc<Self>,
        _callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(StallStore);

fn make_cas_server_with_stall_store(delay: Duration) -> Result<CasServer, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store("main_cas", Store::new(Arc::new(StallStore { delay })));
    CasServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: nativelink_config::cas_server::CasStoreConfig {
                cas_store: "main_cas".to_string(),
                experimental_chunking: None,
            },
        }],
        &store_manager,
    )
}

#[nativelink_test(start_paused = true)]
async fn batch_update_blobs_per_blob_timeout_returns_deadline_exceeded()
-> Result<(), Box<dyn core::error::Error>> {
    const VALUE: &str = "1";

    // Stall longer than `BATCH_PER_BLOB_TIMEOUT` (30 s) so the
    // per-blob timeout fires before the store ever resolves.
    let cas_server = make_cas_server_with_stall_store(Duration::from_secs(120))?;

    let digest = Digest {
        hash: HASH1.to_string(),
        size_bytes: VALUE.len() as i64,
    };
    let raw_response = cas_server
        .batch_update_blobs(Request::new(BatchUpdateBlobsRequest {
            instance_name: INSTANCE_NAME.to_string(),
            requests: vec![batch_update_blobs_request::Request {
                digest: Some(digest.clone()),
                data: VALUE.into(),
                compressor: compressor::Value::Identity.into(),
            }],
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await;

    let response = raw_response.unwrap().into_inner();
    assert_eq!(response.responses.len(), 1);
    let entry = &response.responses[0];
    assert_eq!(entry.digest.as_ref(), Some(&digest));
    let status = entry.status.as_ref().expect("status set");
    assert_eq!(
        status.code,
        Code::DeadlineExceeded as i32,
        "expected DeadlineExceeded, got status: {status:?}",
    );
    assert!(
        status.message.contains("BatchUpdateBlobs per-blob timeout"),
        "unexpected message: {}",
        status.message,
    );
    Ok(())
}

#[nativelink_test(start_paused = true)]
async fn batch_read_blobs_per_blob_timeout_returns_deadline_exceeded()
-> Result<(), Box<dyn core::error::Error>> {
    let cas_server = make_cas_server_with_stall_store(Duration::from_secs(120))?;

    let digest = Digest {
        hash: HASH1.to_string(),
        size_bytes: 1,
    };
    let raw_response = cas_server
        .batch_read_blobs(Request::new(BatchReadBlobsRequest {
            instance_name: INSTANCE_NAME.to_string(),
            digests: vec![digest.clone()],
            acceptable_compressors: vec![compressor::Value::Identity.into()],
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await;

    let response = raw_response.unwrap().into_inner();
    assert_eq!(response.responses.len(), 1);
    let entry = &response.responses[0];
    assert_eq!(entry.digest.as_ref(), Some(&digest));
    assert!(
        entry.data.is_empty(),
        "no data should be returned on timeout"
    );
    let status = entry.status.as_ref().expect("status set");
    assert_eq!(
        status.code,
        Code::DeadlineExceeded as i32,
        "expected DeadlineExceeded, got status: {status:?}",
    );
    assert!(
        status.message.contains("BatchReadBlobs per-blob timeout"),
        "unexpected message: {}",
        status.message,
    );
    Ok(())
}

const CHUNK1_VALUE: &str = "hello ";
const CHUNK2_VALUE: &str = "world";

async fn make_chunking_store_manager() -> Result<Arc<StoreManager>, Error> {
    let store_manager = make_store_manager().await?;
    store_manager.add_store(
        "chunk_index",
        store_factory(
            &StoreSpec::Memory(MemorySpec::default()),
            &store_manager,
            None,
        )
        .await?,
    );
    Ok(store_manager)
}

fn make_chunking_cas_server(store_manager: &StoreManager) -> Result<CasServer, Error> {
    make_chunking_cas_server_with_avg(store_manager, 0)
}

fn make_chunking_cas_server_with_avg(
    store_manager: &StoreManager,
    avg_chunk_size_bytes: u64,
) -> Result<CasServer, Error> {
    CasServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: nativelink_config::cas_server::CasStoreConfig {
                cas_store: "main_cas".to_string(),
                experimental_chunking: Some(nativelink_config::cas_server::CasChunkingConfig {
                    index_store: Some("chunk_index".to_string()),
                    avg_chunk_size_bytes,
                }),
            },
        }],
        store_manager,
    )
}

/// Uploads the two test chunks to the store and returns their digests and
/// the digest of their concatenation.
async fn upload_test_chunks(store: &Store) -> Result<(Digest, Digest, Digest), Error> {
    let chunk1_digest = Digest {
        hash: HASH1.to_string(),
        size_bytes: CHUNK1_VALUE.len() as i64,
    };
    let chunk2_digest = Digest {
        hash: HASH2.to_string(),
        size_bytes: CHUNK2_VALUE.len() as i64,
    };
    store
        .update_oneshot(
            DigestInfo::try_from(chunk1_digest.clone())?,
            CHUNK1_VALUE.into(),
        )
        .await?;
    store
        .update_oneshot(
            DigestInfo::try_from(chunk2_digest.clone())?,
            CHUNK2_VALUE.into(),
        )
        .await?;
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(CHUNK1_VALUE.as_bytes());
    hasher.update(CHUNK2_VALUE.as_bytes());
    let blob_digest: Digest = hasher.finalize_digest().into();
    Ok((chunk1_digest, chunk2_digest, blob_digest))
}

#[nativelink_test]
async fn splice_and_split_round_trip() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_chunking_store_manager().await?;
    let cas_server = make_chunking_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    let (chunk1_digest, chunk2_digest, blob_digest) = upload_test_chunks(&store).await?;

    let splice_response = cas_server
        .splice_blob(Request::new(SpliceBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(blob_digest.clone()),
            chunk_digests: vec![chunk1_digest.clone(), chunk2_digest.clone()],
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await?
        .into_inner();
    assert_eq!(splice_response.blob_digest.as_ref(), Some(&blob_digest));

    // The spliced blob must be materialized in the CAS so non-chunking
    // clients can read it.
    let blob_data = store
        .get_part_unchunked(DigestInfo::try_from(blob_digest.clone())?, 0, None)
        .await?;
    assert_eq!(blob_data, format!("{CHUNK1_VALUE}{CHUNK2_VALUE}"));

    let split_response = cas_server
        .split_blob(Request::new(SplitBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(blob_digest),
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await?
        .into_inner();
    assert_eq!(
        split_response.chunk_digests,
        vec![chunk1_digest, chunk2_digest]
    );
    assert_eq!(
        split_response.chunking_function,
        i32::from(chunking_function::Value::FastCdc2020)
    );

    let metrics = cas_server.chunking_metrics();
    assert_eq!(metrics.splice_requests_total.load(Ordering::Relaxed), 1);
    assert_eq!(
        metrics.splice_bytes_total.load(Ordering::Relaxed),
        (CHUNK1_VALUE.len() + CHUNK2_VALUE.len()) as u64
    );
    assert_eq!(metrics.split_requests_total.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.split_hits.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.split_misses.load(Ordering::Relaxed), 0);
    Ok(())
}

#[nativelink_test]
async fn splice_blob_rejects_digest_mismatch() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_chunking_store_manager().await?;
    let cas_server = make_chunking_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    let (chunk1_digest, chunk2_digest, _blob_digest) = upload_test_chunks(&store).await?;
    let total_size = chunk1_digest.size_bytes + chunk2_digest.size_bytes;
    let wrong_blob_digest = Digest {
        hash: HASH3.to_string(),
        size_bytes: total_size,
    };

    let status = cas_server
        .splice_blob(Request::new(SpliceBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(wrong_blob_digest.clone()),
            chunk_digests: vec![chunk1_digest, chunk2_digest],
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status
            .message()
            .contains("does not match the expected digest"),
        "unexpected message: {}",
        status.message()
    );

    // The blob must not have been committed to the CAS.
    let blob_exists = store.has(DigestInfo::try_from(wrong_blob_digest)?).await?;
    assert_eq!(blob_exists, None);
    assert_eq!(
        cas_server
            .chunking_metrics()
            .splice_verification_failures
            .load(Ordering::Relaxed),
        1
    );
    Ok(())
}

#[nativelink_test]
async fn splice_blob_rejects_size_mismatch() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_chunking_store_manager().await?;
    let cas_server = make_chunking_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    let (chunk1_digest, chunk2_digest, blob_digest) = upload_test_chunks(&store).await?;
    let wrong_blob_digest = Digest {
        size_bytes: blob_digest.size_bytes + 1,
        ..blob_digest
    };

    let status = cas_server
        .splice_blob(Request::new(SpliceBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(wrong_blob_digest),
            chunk_digests: vec![chunk1_digest, chunk2_digest],
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status
            .message()
            .contains("does not match the expected blob size"),
        "unexpected message: {}",
        status.message()
    );
    Ok(())
}

#[nativelink_test]
async fn splice_blob_missing_chunk_returns_not_found() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_chunking_store_manager().await?;
    let cas_server = make_chunking_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    // Only upload the first chunk.
    let chunk1_digest = Digest {
        hash: HASH1.to_string(),
        size_bytes: CHUNK1_VALUE.len() as i64,
    };
    store
        .update_oneshot(
            DigestInfo::try_from(chunk1_digest.clone())?,
            CHUNK1_VALUE.into(),
        )
        .await?;
    let missing_chunk_digest = Digest {
        hash: HASH2.to_string(),
        size_bytes: CHUNK2_VALUE.len() as i64,
    };
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(CHUNK1_VALUE.as_bytes());
    hasher.update(CHUNK2_VALUE.as_bytes());
    let blob_digest: Digest = hasher.finalize_digest().into();

    let status = cas_server
        .splice_blob(Request::new(SpliceBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(blob_digest),
            chunk_digests: vec![chunk1_digest, missing_chunk_digest],
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::NotFound);
    Ok(())
}

#[nativelink_test]
async fn split_blob_absent_blob_returns_not_found() -> Result<(), Box<dyn core::error::Error>> {
    const VALUE: &str = "1";

    let store_manager = make_chunking_store_manager().await?;
    let cas_server = make_chunking_cas_server(&store_manager)?;

    // The blob was never uploaded.
    let status = cas_server
        .split_blob(Request::new(SplitBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(Digest {
                hash: HASH1.to_string(),
                size_bytes: VALUE.len() as i64,
            }),
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::NotFound);
    let metrics = cas_server.chunking_metrics();
    assert_eq!(metrics.split_requests_total.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.split_hits.load(Ordering::Relaxed), 0);
    assert_eq!(metrics.split_misses.load(Ordering::Relaxed), 1);
    Ok(())
}

#[nativelink_test]
async fn split_and_splice_disabled_return_unimplemented() -> Result<(), Box<dyn core::error::Error>>
{
    const VALUE: &str = "1";

    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;

    let digest = Digest {
        hash: HASH1.to_string(),
        size_bytes: VALUE.len() as i64,
    };
    let split_status = cas_server
        .split_blob(Request::new(SplitBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(digest.clone()),
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(split_status.code(), Code::Unimplemented);

    let splice_status = cas_server
        .splice_blob(Request::new(SpliceBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(digest.clone()),
            chunk_digests: vec![digest],
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(splice_status.code(), Code::Unimplemented);
    Ok(())
}

#[nativelink_test]
async fn split_blob_chunks_small_blob_on_demand() -> Result<(), Box<dyn core::error::Error>> {
    const VALUE: &str = "1";

    let store_manager = make_chunking_store_manager().await?;
    let cas_server = make_chunking_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    // Upload the blob whole (as a remote execution worker would) under its
    // real digest, without ever calling SpliceBlob.
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(VALUE.as_bytes());
    let blob_digest: Digest = hasher.finalize_digest().into();
    store
        .update_oneshot(DigestInfo::try_from(blob_digest.clone())?, VALUE.into())
        .await?;

    let split_response = cas_server
        .split_blob(Request::new(SplitBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(blob_digest.clone()),
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await?
        .into_inner();
    // A blob smaller than the minimum chunk size is a single chunk whose
    // digest equals the blob digest.
    assert_eq!(split_response.chunk_digests, vec![blob_digest]);
    assert_eq!(
        split_response.chunking_function,
        i32::from(chunking_function::Value::FastCdc2020)
    );
    let metrics = cas_server.chunking_metrics();
    assert_eq!(metrics.split_chunked_on_demand.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.split_hits.load(Ordering::Relaxed), 0);
    Ok(())
}

#[nativelink_test]
async fn split_blob_chunks_large_blob_on_demand_and_reuses_layout()
-> Result<(), Box<dyn core::error::Error>> {
    // Use the smallest allowed average (1 KiB -> min 256, max 4096) so a
    // small test blob still produces multiple chunks.
    const AVG_CHUNK_SIZE: u64 = 1024;
    const BLOB_SIZE: usize = 16 * 1024;

    let store_manager = make_chunking_store_manager().await?;
    let cas_server = make_chunking_cas_server_with_avg(&store_manager, AVG_CHUNK_SIZE)?;
    let store = store_manager.get_store("main_cas").unwrap();

    // Deterministic pseudo-random content so FastCDC finds content-defined
    // boundaries.
    let mut state = 0x9e37_79b9_u32;
    let data: Vec<u8> = (0..BLOB_SIZE)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect();
    let blob_digest = Digest {
        hash: HASH1.to_string(),
        size_bytes: BLOB_SIZE as i64,
    };
    store
        .update_oneshot(
            DigestInfo::try_from(blob_digest.clone())?,
            bytes::Bytes::from(data.clone()),
        )
        .await?;

    let split_response = cas_server
        .split_blob(Request::new(SplitBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(blob_digest.clone()),
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await?
        .into_inner();
    assert!(
        split_response.chunk_digests.len() > 1,
        "expected multiple chunks, got {}",
        split_response.chunk_digests.len()
    );

    // All chunks must be stored in the CAS and concatenate to the original
    // blob in order.
    let mut reassembled = Vec::with_capacity(BLOB_SIZE);
    for chunk_digest in &split_response.chunk_digests {
        let chunk_data = store
            .get_part_unchunked(DigestInfo::try_from(chunk_digest.clone())?, 0, None)
            .await?;
        reassembled.extend_from_slice(&chunk_data);
    }
    assert_eq!(reassembled, data);

    // A second split must be served from the stored layout.
    let second_response = cas_server
        .split_blob(Request::new(SplitBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(blob_digest),
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await?
        .into_inner();
    assert_eq!(second_response.chunk_digests, split_response.chunk_digests);
    let metrics = cas_server.chunking_metrics();
    assert_eq!(metrics.split_requests_total.load(Ordering::Relaxed), 2);
    assert_eq!(metrics.split_chunked_on_demand.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.split_hits.load(Ordering::Relaxed), 1);
    Ok(())
}

#[nativelink_test]
async fn split_blob_falls_back_when_layout_unusable() -> Result<(), Box<dyn core::error::Error>> {
    const VALUE: &str = "1";

    let store_manager = make_chunking_store_manager().await?;
    let cas_server = make_chunking_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();
    let index_store = store_manager.get_store("chunk_index").unwrap();

    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(VALUE.as_bytes());
    let blob_digest: Digest = hasher.finalize_digest().into();
    store
        .update_oneshot(DigestInfo::try_from(blob_digest.clone())?, VALUE.into())
        .await?;

    // Register a layout whose only chunk is not present in the CAS,
    // simulating a chunk that was evicted after the layout was stored.
    let stale_layout = SplitBlobResponse {
        chunk_digests: vec![Digest {
            hash: HASH2.to_string(),
            size_bytes: VALUE.len() as i64,
        }],
        chunking_function: chunking_function::Value::FastCdc2020.into(),
    };
    index_store
        .update_oneshot(
            DigestInfo::try_from(blob_digest.clone())?,
            stale_layout.encode_to_vec().into(),
        )
        .await?;

    // The unusable layout must be ignored and the blob re-chunked on demand.
    let split_response = cas_server
        .split_blob(Request::new(SplitBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(blob_digest.clone()),
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await?
        .into_inner();
    assert_eq!(split_response.chunk_digests, vec![blob_digest]);
    let metrics = cas_server.chunking_metrics();
    assert_eq!(metrics.split_hits.load(Ordering::Relaxed), 0);
    assert_eq!(metrics.split_chunked_on_demand.load(Ordering::Relaxed), 1);
    Ok(())
}

#[nativelink_test]
async fn chunking_rejects_index_store_same_as_cas_store() -> Result<(), Box<dyn core::error::Error>>
{
    let store_manager = make_store_manager().await?;
    let error = CasServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: nativelink_config::cas_server::CasStoreConfig {
                cas_store: "main_cas".to_string(),
                experimental_chunking: Some(nativelink_config::cas_server::CasChunkingConfig {
                    index_store: Some("main_cas".to_string()),
                    avg_chunk_size_bytes: 0,
                }),
            },
        }],
        &store_manager,
    )
    .err()
    .expect("expected same-store index_store to be rejected");
    assert!(
        error
            .to_string()
            .contains("must not be the same store as 'cas_store'"),
        "unexpected error: {error}"
    );
    Ok(())
}

#[nativelink_test]
async fn chunking_on_grpc_store_forbids_index_store() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        "grpc_cas",
        store_factory(
            &StoreSpec::Grpc(nativelink_config::stores::GrpcSpec {
                instance_name: "backend".to_string(),
                endpoints: vec![nativelink_config::stores::GrpcEndpoint {
                    address: "http://localhost:1".to_string(),
                    tls_config: None,
                    concurrency_limit: None,
                    connect_timeout_s: 0,
                    tcp_keepalive_s: 0,
                    http2_keepalive_interval_s: 0,
                    http2_keepalive_timeout_s: 0,
                }],
                store_type: nativelink_config::stores::StoreType::Cas,
                retry: nativelink_config::stores::Retry::default(),
                max_concurrent_requests: 0,
                connections_per_endpoint: 0,
                rpc_timeout_s: 1,
                use_legacy_resource_names: false,
                headers: std::collections::HashMap::new(),
                forward_headers: vec![],
            }),
            &store_manager,
            None,
        )
        .await?,
    );

    let make_config = |index_store: Option<String>| {
        vec![WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: nativelink_config::cas_server::CasStoreConfig {
                cas_store: "grpc_cas".to_string(),
                experimental_chunking: Some(nativelink_config::cas_server::CasChunkingConfig {
                    index_store,
                    avg_chunk_size_bytes: 0,
                }),
            },
        }]
    };

    // A local index store is meaningless when the RPCs are forwarded.
    let error = CasServer::new(&make_config(Some("grpc_cas".to_string())), &store_manager)
        .err()
        .expect("expected index_store on grpc store to be rejected");
    assert!(
        error.to_string().contains("must not be set"),
        "unexpected error: {error}"
    );

    // Without an index_store the configuration is valid: SplitBlob and
    // SpliceBlob are forwarded to the backend.
    CasServer::new(&make_config(None), &store_manager)?;
    Ok(())
}
