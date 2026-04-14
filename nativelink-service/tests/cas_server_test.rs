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
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use nativelink_config::cas_server::WithInstanceName;
use nativelink_config::stores::{MemorySpec, StoreSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_server::ContentAddressableStorage;
use nativelink_proto::build::bazel::remote::execution::v2::{
    BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, Digest, Directory, DirectoryNode,
    FindMissingBlobsRequest, GetTreeRequest, GetTreeResponse, NodeProperties,
    batch_read_blobs_response, batch_update_blobs_request, batch_update_blobs_response,
    compressor, digest_function,
};
use nativelink_proto::google::rpc::Status as GrpcStatus;
use nativelink_service::cas_server::CasServer;
use nativelink_store::ac_utils::serialize_and_upload_message;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::store_trait::{StoreKey, StoreLike};
use pretty_assertions::assert_eq;
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

// ---------------------------------------------------------------------------
// Helper: collect all directories from a GetTree streaming response.
// ---------------------------------------------------------------------------

async fn collect_get_tree_dirs(
    cas_server: &CasServer,
    root_digest_info: DigestInfo,
    page_size: i32,
) -> Vec<Directory> {
    let raw_response = cas_server
        .get_tree(Request::new(GetTreeRequest {
            instance_name: INSTANCE_NAME.to_string(),
            page_size,
            page_token: String::new(),
            root_digest: Some(root_digest_info.into()),
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await
        .expect("get_tree should succeed");
    raw_response
        .into_inner()
        .filter_map(|x| async move { Some(x.unwrap()) })
        .flat_map(|resp| futures::stream::iter(resp.directories))
        .collect::<Vec<_>>()
        .await
}

// ---------------------------------------------------------------------------
// Helper: upload a Directory proto and return its DigestInfo.
// ---------------------------------------------------------------------------

async fn upload_directory(
    store: Pin<&impl StoreLike>,
    directory: &Directory,
) -> Result<DigestInfo, Error> {
    serialize_and_upload_message(
        directory,
        store,
        &mut DigestHasherFunc::Sha256.hasher(),
    )
    .await
}

// ===========================================================================
// Test 1: tree_cache_hit
// Verifies that a second unpaginated GetTree call for the same root is
// served from the tree cache (correct result AND faster).
// ===========================================================================

#[nativelink_test]
async fn tree_cache_hit() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    let result = setup_directory_structure(store.as_pin()).await?;

    // First call: populates the tree cache.
    let first_start = Instant::now();
    let first_dirs = collect_get_tree_dirs(&cas_server, result.root_directory_digest_info, 0).await;
    let first_elapsed = first_start.elapsed();

    // Verify the tree cache was populated.
    assert_eq!(
        cas_server.tree_cache_len().await,
        1,
        "tree cache should have exactly 1 entry after first call"
    );

    // Second call: should hit the tree cache.
    let second_start = Instant::now();
    let second_dirs =
        collect_get_tree_dirs(&cas_server, result.root_directory_digest_info, 0).await;
    let second_elapsed = second_start.elapsed();

    // Both calls must return the same directories.
    assert_eq!(first_dirs, second_dirs, "cache hit should return same data");

    // Verify the expected directory count: root + 5 sub-directories.
    assert_eq!(first_dirs.len(), 6);

    // The cache hit should still show 1 entry (not 2).
    assert_eq!(
        cas_server.tree_cache_len().await,
        1,
        "tree cache should still have exactly 1 entry"
    );

    // Cache hit should be significantly faster than BFS traversal.
    assert!(
        second_elapsed < first_elapsed || second_elapsed.as_micros() < 500,
        "cache hit ({second_elapsed:?}) should be faster than BFS ({first_elapsed:?})"
    );

    Ok(())
}

// ===========================================================================
// Test 2: tree_cache_miss_different_root
// Verifies that different root digests produce independent cache entries
// with correct results.
// ===========================================================================

#[nativelink_test]
async fn tree_cache_miss_different_root() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    // Build tree A: root_a -> [child_a1, child_a2]
    let child_a1 = Directory {
        node_properties: Some(NodeProperties {
            mtime: Some(Timestamp { seconds: 1, nanos: 0 }),
            unix_mode: Some(0o755),
            ..Default::default()
        }),
        ..Default::default()
    };
    let child_a1_digest = upload_directory(store.as_pin(), &child_a1).await?;

    let child_a2 = Directory {
        node_properties: Some(NodeProperties {
            mtime: Some(Timestamp { seconds: 2, nanos: 0 }),
            unix_mode: Some(0o755),
            ..Default::default()
        }),
        ..Default::default()
    };
    let child_a2_digest = upload_directory(store.as_pin(), &child_a2).await?;

    let root_a = Directory {
        directories: vec![
            DirectoryNode {
                name: "a1".into(),
                digest: Some(child_a1_digest.into()),
            },
            DirectoryNode {
                name: "a2".into(),
                digest: Some(child_a2_digest.into()),
            },
        ],
        ..Default::default()
    };
    let root_a_digest = upload_directory(store.as_pin(), &root_a).await?;

    // Build tree B: root_b -> [child_b1]
    let child_b1 = Directory {
        node_properties: Some(NodeProperties {
            mtime: Some(Timestamp { seconds: 99, nanos: 0 }),
            unix_mode: Some(0o700),
            ..Default::default()
        }),
        ..Default::default()
    };
    let child_b1_digest = upload_directory(store.as_pin(), &child_b1).await?;

    let root_b = Directory {
        directories: vec![DirectoryNode {
            name: "b1".into(),
            digest: Some(child_b1_digest.into()),
        }],
        ..Default::default()
    };
    let root_b_digest = upload_directory(store.as_pin(), &root_b).await?;

    // Fetch tree A.
    let dirs_a = collect_get_tree_dirs(&cas_server, root_a_digest, 0).await;
    assert_eq!(dirs_a.len(), 3, "tree A: root + 2 children");
    assert_eq!(dirs_a[0], root_a);
    assert_eq!(dirs_a[1], child_a1);
    assert_eq!(dirs_a[2], child_a2);

    // Fetch tree B.
    let dirs_b = collect_get_tree_dirs(&cas_server, root_b_digest, 0).await;
    assert_eq!(dirs_b.len(), 2, "tree B: root + 1 child");
    assert_eq!(dirs_b[0], root_b);
    assert_eq!(dirs_b[1], child_b1);

    // Both trees should be cached independently.
    assert_eq!(
        cas_server.tree_cache_len().await,
        2,
        "tree cache should have 2 independent entries"
    );

    // Re-fetch tree A and verify it still returns the correct data.
    let dirs_a_again = collect_get_tree_dirs(&cas_server, root_a_digest, 0).await;
    assert_eq!(dirs_a, dirs_a_again, "tree A cache hit returns same data");

    Ok(())
}

// ===========================================================================
// Test 3: subtree_cache_overlap
// Two trees that share a common subdirectory subtree. The second GetTree
// call should benefit from the subtree cache populated by the first call.
// ===========================================================================

#[nativelink_test]
async fn subtree_cache_overlap() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    // Shared subtree: shared_child (a leaf directory).
    let shared_child = Directory {
        node_properties: Some(NodeProperties {
            mtime: Some(Timestamp { seconds: 42, nanos: 0 }),
            unix_mode: Some(0o755),
            ..Default::default()
        }),
        ..Default::default()
    };
    let shared_child_digest = upload_directory(store.as_pin(), &shared_child).await?;

    // Tree X: root_x -> [shared_child, unique_x_child]
    let unique_x_child = Directory {
        node_properties: Some(NodeProperties {
            mtime: Some(Timestamp { seconds: 10, nanos: 0 }),
            unix_mode: Some(0o755),
            ..Default::default()
        }),
        ..Default::default()
    };
    let unique_x_digest = upload_directory(store.as_pin(), &unique_x_child).await?;

    let root_x = Directory {
        directories: vec![
            DirectoryNode {
                name: "shared".into(),
                digest: Some(shared_child_digest.into()),
            },
            DirectoryNode {
                name: "unique_x".into(),
                digest: Some(unique_x_digest.into()),
            },
        ],
        ..Default::default()
    };
    let root_x_digest = upload_directory(store.as_pin(), &root_x).await?;

    // Tree Y: root_y -> [shared_child, unique_y_child]
    let unique_y_child = Directory {
        node_properties: Some(NodeProperties {
            mtime: Some(Timestamp { seconds: 20, nanos: 0 }),
            unix_mode: Some(0o755),
            ..Default::default()
        }),
        ..Default::default()
    };
    let unique_y_digest = upload_directory(store.as_pin(), &unique_y_child).await?;

    let root_y = Directory {
        directories: vec![
            DirectoryNode {
                name: "shared".into(),
                digest: Some(shared_child_digest.into()),
            },
            DirectoryNode {
                name: "unique_y".into(),
                digest: Some(unique_y_digest.into()),
            },
        ],
        ..Default::default()
    };
    let root_y_digest = upload_directory(store.as_pin(), &root_y).await?;

    // Fetch tree X first: populates subtree cache for all 3 directories
    // (root_x, shared_child, unique_x_child).
    let dirs_x = collect_get_tree_dirs(&cas_server, root_x_digest, 0).await;
    assert_eq!(dirs_x.len(), 3);
    assert_eq!(dirs_x[0], root_x);

    // The subtree cache should have entries for root_x's directories.
    let subtree_len_after_x = cas_server.subtree_cache_len().await;
    assert!(
        subtree_len_after_x >= 3,
        "subtree cache should have at least 3 entries (root_x + 2 children), got {subtree_len_after_x}"
    );

    // Fetch tree Y: shared_child should come from subtree cache.
    let dirs_y = collect_get_tree_dirs(&cas_server, root_y_digest, 0).await;
    assert_eq!(dirs_y.len(), 3);
    assert_eq!(dirs_y[0], root_y);

    // Verify both trees return their shared child correctly.
    assert!(
        dirs_x.contains(&shared_child),
        "tree X should contain the shared child"
    );
    assert!(
        dirs_y.contains(&shared_child),
        "tree Y should contain the shared child"
    );

    // Subtree cache should now have entries for all unique directories
    // across both trees. The shared_child is counted once.
    let subtree_len_after_y = cas_server.subtree_cache_len().await;
    // root_x, shared_child, unique_x, root_y, unique_y = 5 unique digests
    assert!(
        subtree_len_after_y >= 5,
        "subtree cache should have at least 5 entries after both trees, got {subtree_len_after_y}"
    );

    Ok(())
}

// ===========================================================================
// Test 4: coalescing_concurrent
// Spawns multiple concurrent GetTree calls for the same root. Verifies
// all return the same result and only 1 tree cache entry is created.
// ===========================================================================

#[nativelink_test]
async fn coalescing_concurrent() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = Arc::new(make_cas_server(&store_manager)?);
    let store = store_manager.get_store("main_cas").unwrap();

    let result = setup_directory_structure(store.as_pin()).await?;
    let root_digest_info = result.root_directory_digest_info;

    // Build expected directories list for comparison.
    let mut expected_dirs = vec![result.root_directory.clone()];
    expected_dirs.extend(result.sub_directories.iter().cloned());

    // Spawn 10 concurrent GetTree calls.
    let mut handles = Vec::with_capacity(10);
    for _ in 0..10 {
        let server = cas_server.clone();
        let handle = tokio::spawn(async move {
            let raw_response = server
                .get_tree(Request::new(GetTreeRequest {
                    instance_name: INSTANCE_NAME.to_string(),
                    page_size: 0,
                    page_token: String::new(),
                    root_digest: Some(root_digest_info.into()),
                    digest_function: digest_function::Value::Sha256.into(),
                }))
                .await
                .expect("get_tree should succeed");
            raw_response
                .into_inner()
                .filter_map(|x| async move { Some(x.unwrap()) })
                .flat_map(|resp| futures::stream::iter(resp.directories))
                .collect::<Vec<_>>()
                .await
        });
        handles.push(handle);
    }

    // Collect all results.
    let mut results = Vec::with_capacity(10);
    for handle in handles {
        results.push(handle.await?);
    }

    // All 10 calls must return the same correct directories.
    for (i, dirs) in results.iter().enumerate() {
        assert_eq!(
            *dirs, expected_dirs,
            "concurrent call {i} returned wrong directories"
        );
    }

    // The tree cache should have exactly 1 entry, not 10.
    assert_eq!(
        cas_server.tree_cache_len().await,
        1,
        "coalescing should result in exactly 1 tree cache entry"
    );

    // No in-flight entries should remain after all calls complete.
    assert_eq!(
        cas_server.tree_inflight_len(),
        0,
        "no in-flight entries should remain after completion"
    );

    Ok(())
}

// ===========================================================================
// Test 5: coalescing_leader_failure
// When the leader BFS fails (missing root directory), waiters wake up
// and perform their own BFS. No deadlock should occur.
// ===========================================================================

#[nativelink_test]
async fn coalescing_leader_failure() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = Arc::new(make_cas_server(&store_manager)?);

    // Use a digest that does NOT exist in the store. The BFS will fail to
    // find the root directory. This tests that the leader properly signals
    // waiters even on failure, and no deadlock occurs.
    let missing_digest = DigestInfo::try_new(HASH1, 100)?;

    // Spawn 2 concurrent calls for the missing root.
    let mut handles = Vec::with_capacity(2);
    for _ in 0..2 {
        let server = cas_server.clone();
        handles.push(tokio::spawn(async move {
            let raw_response = server
                .get_tree(Request::new(GetTreeRequest {
                    instance_name: INSTANCE_NAME.to_string(),
                    page_size: 0,
                    page_token: String::new(),
                    root_digest: Some(missing_digest.into()),
                    digest_function: digest_function::Value::Sha256.into(),
                }))
                .await;
            // The call should succeed (GetTree returns a stream), but the
            // stream should yield a response with an empty directory list
            // (the root was missing, so BFS traversal produces nothing).
            match raw_response {
                Ok(resp) => {
                    let responses: Vec<_> = resp
                        .into_inner()
                        .filter_map(|x| async move { x.ok() })
                        .collect()
                        .await;
                    responses
                }
                Err(_status) => {
                    // An error status is also acceptable — the root doesn't exist.
                    vec![]
                }
            }
        }));
    }

    // All tasks should complete without deadlock. Use a timeout to detect
    // deadlock.
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        for handle in handles {
            let _result = handle.await.expect("task should not panic");
        }
    })
    .await;
    assert!(
        timeout.is_ok(),
        "coalescing with leader failure should not deadlock"
    );

    // No in-flight entries should remain.
    assert_eq!(
        cas_server.tree_inflight_len(),
        0,
        "no in-flight entries should remain after failure"
    );

    // The tree cache should NOT have an entry because the BFS had missing
    // directories (total_missing_skipped > 0 prevents caching).
    assert_eq!(
        cas_server.tree_cache_len().await,
        0,
        "failed BFS should not populate tree cache"
    );

    Ok(())
}

// ===========================================================================
// Test 6: paginated_bypasses_cache
// Paginated GetTree calls (page_size > 0) should NOT cache results in
// the tree cache. A subsequent unpaginated call should do a fresh BFS.
// ===========================================================================

#[nativelink_test]
async fn paginated_bypasses_cache() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    let result = setup_directory_structure(store.as_pin()).await?;

    // Make a paginated GetTree call (page_size = 2).
    let _paginated_dirs =
        collect_get_tree_dirs(&cas_server, result.root_directory_digest_info, 2).await;

    // The tree cache should NOT have been populated by a paginated call.
    assert_eq!(
        cas_server.tree_cache_len().await,
        0,
        "paginated GetTree should not populate tree cache"
    );

    // Now make an unpaginated call — it should do a fresh BFS and cache.
    let unpaginated_dirs =
        collect_get_tree_dirs(&cas_server, result.root_directory_digest_info, 0).await;
    assert_eq!(unpaginated_dirs.len(), 6, "unpaginated should return all 6 directories");

    assert_eq!(
        cas_server.tree_cache_len().await,
        1,
        "unpaginated GetTree should populate tree cache"
    );

    Ok(())
}

// ===========================================================================
// Test 7: subtree_cache_deduplication
// Verifies that when a tree has duplicate subtrees (same digest referenced
// by multiple parents), the BFS correctly deduplicates them and the
// subtree cache stores each unique directory exactly once.
// ===========================================================================

#[nativelink_test]
async fn subtree_cache_deduplication() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    // Create a shared leaf directory.
    let shared_leaf = Directory {
        node_properties: Some(NodeProperties {
            mtime: Some(Timestamp { seconds: 7, nanos: 0 }),
            unix_mode: Some(0o755),
            ..Default::default()
        }),
        ..Default::default()
    };
    let shared_leaf_digest = upload_directory(store.as_pin(), &shared_leaf).await?;

    // Create two mid-level directories that both reference the shared leaf.
    let mid_a = Directory {
        directories: vec![DirectoryNode {
            name: "leaf".into(),
            digest: Some(shared_leaf_digest.into()),
        }],
        ..Default::default()
    };
    let mid_a_digest = upload_directory(store.as_pin(), &mid_a).await?;

    let mid_b = Directory {
        directories: vec![DirectoryNode {
            name: "leaf".into(),
            digest: Some(shared_leaf_digest.into()),
        }],
        ..Default::default()
    };
    let mid_b_digest = upload_directory(store.as_pin(), &mid_b).await?;

    // Root references both mid-level directories.
    let root = Directory {
        directories: vec![
            DirectoryNode {
                name: "mid_a".into(),
                digest: Some(mid_a_digest.into()),
            },
            DirectoryNode {
                name: "mid_b".into(),
                digest: Some(mid_b_digest.into()),
            },
        ],
        ..Default::default()
    };
    let root_digest = upload_directory(store.as_pin(), &root).await?;

    let dirs = collect_get_tree_dirs(&cas_server, root_digest, 0).await;

    // BFS should return: root, mid_a, mid_b, shared_leaf.
    // Note: mid_a and mid_b have the SAME content but different names at
    // the parent level. However, since Directory proto content is
    // identical, they have the same digest and will be deduplicated.
    // Actually, mid_a and mid_b are structurally identical (same
    // directories field), so they'll have the same digest. Let's check.
    assert_eq!(
        mid_a_digest, mid_b_digest,
        "mid_a and mid_b have identical content, so same digest"
    );

    // With deduplication, we get: root, mid_a (=mid_b), shared_leaf = 3.
    assert_eq!(dirs.len(), 3, "deduplication should yield 3 unique directories");
    assert_eq!(dirs[0], root);

    // Subtree cache should have 3 unique entries.
    let subtree_len = cas_server.subtree_cache_len().await;
    assert_eq!(
        subtree_len, 3,
        "subtree cache should have 3 unique entries"
    );

    Ok(())
}

// ===========================================================================
// Test 8: tree_cache_returns_correct_next_page_token
// Verifies that cached GetTree results preserve the next_page_token
// (empty string for complete trees).
// ===========================================================================

#[nativelink_test]
async fn tree_cache_returns_correct_next_page_token() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let cas_server = make_cas_server(&store_manager)?;
    let store = store_manager.get_store("main_cas").unwrap();

    let result = setup_directory_structure(store.as_pin()).await?;

    // First call: populates cache.
    let raw_response = cas_server
        .get_tree(Request::new(GetTreeRequest {
            instance_name: INSTANCE_NAME.to_string(),
            page_size: 0,
            page_token: String::new(),
            root_digest: Some(result.root_directory_digest_info.into()),
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await?;
    let first_responses: Vec<GetTreeResponse> = raw_response
        .into_inner()
        .filter_map(|x| async move { Some(x.unwrap()) })
        .collect()
        .await;
    assert_eq!(first_responses.len(), 1);
    assert_eq!(
        first_responses[0].next_page_token, "",
        "complete tree should have empty next_page_token"
    );

    // Second call: from cache. Should also have empty next_page_token.
    let raw_response = cas_server
        .get_tree(Request::new(GetTreeRequest {
            instance_name: INSTANCE_NAME.to_string(),
            page_size: 0,
            page_token: String::new(),
            root_digest: Some(result.root_directory_digest_info.into()),
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await?;
    let second_responses: Vec<GetTreeResponse> = raw_response
        .into_inner()
        .filter_map(|x| async move { Some(x.unwrap()) })
        .collect()
        .await;
    assert_eq!(second_responses.len(), 1);
    assert_eq!(
        second_responses[0].next_page_token, "",
        "cached result should preserve empty next_page_token"
    );

    // Verify the full response structure matches.
    assert_eq!(first_responses, second_responses);

    Ok(())
}
