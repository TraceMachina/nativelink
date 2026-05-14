// Copyright 2026 The NativeLink Authors. All rights reserved.
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

use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use nativelink_config::stores::MemorySpec;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::{
    Directory as ProtoDirectory, DirectoryNode, FileNode, SymlinkNode,
};
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::common::{DigestInfo, make_temp_path};
use nativelink_util::store_trait::{Store, StoreLike};
use nativelink_worker::directory_cache::{DirectoryCache, DirectoryCacheConfig};
use prost::Message;
use tonic::Code;
use uuid::Uuid;

#[nativelink_test]
async fn create_directory_cache() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    let store = MemoryStore::new(&MemorySpec::default());
    DirectoryCache::new(config, Store::new(store)).await?;
    assert!(!logs_contain("ERROR"));
    assert!(!logs_contain("WARN"));
    Ok(())
}

#[nativelink_test]
async fn add_missing_file_to_cache() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    let store = MemoryStore::new(&MemorySpec::default());
    let cache = DirectoryCache::new(config, Store::new(store)).await?;
    let digest = DigestInfo::new([1u8; 32], 5);
    let uuid = Uuid::new_v4();
    let res = cache
        .get_or_create(digest, Path::new(&uuid.to_string()))
        .await;
    assert_eq!(res, Err(Error::new_with_messages(Code::NotFound, vec![
        "Key Digest(DigestInfo(\"0101010101010101010101010101010101010101010101010101010101010101-5\")) not found".to_string(),
        "Failed to fetch directory digest: DigestInfo(\"0101010101010101010101010101010101010101010101010101010101010101-5\")".to_string()])));
    assert!(!logs_contain("ERROR"));
    assert!(!logs_contain("WARN"));
    Ok(())
}

async fn single_insert(config: DirectoryCacheConfig) -> Result<(), Error> {
    let digest1 = DigestInfo::new([1u8; 32], 5);
    let store = MemoryStore::new(&MemorySpec::default());
    assert_eq!(
        store.update_oneshot(digest1, Bytes::from_static(b"")).await,
        Ok(())
    );
    let cache = DirectoryCache::new(config, Store::new(store)).await?;
    assert_eq!(
        cache
            .get_or_create(digest1, Path::new(&Uuid::new_v4().to_string()))
            .await,
        Ok(false)
    );
    Ok(())
}

async fn double_insert(config: DirectoryCacheConfig) -> Result<(), Error> {
    let store = MemoryStore::new(&MemorySpec::default());
    double_insert_with_data(
        config,
        store,
        Bytes::from_static(b""),
        Bytes::from_static(b""),
    )
    .await
}

async fn double_insert_with_data(
    config: DirectoryCacheConfig,
    store: Arc<MemoryStore>,
    first: Bytes,
    second: Bytes,
) -> Result<(), Error> {
    let digest1 = DigestInfo::new([1u8; 32], 5);
    let digest2 = DigestInfo::new([2u8; 32], 5);
    assert_eq!(store.update_oneshot(digest1, first).await, Ok(()));
    assert_eq!(store.update_oneshot(digest2, second).await, Ok(()));
    let cache = DirectoryCache::new(config, Store::new(store)).await?;
    assert_eq!(
        cache
            .get_or_create(digest1, Path::new(&Uuid::new_v4().to_string()))
            .await,
        Ok(false)
    );
    assert_eq!(
        cache
            .get_or_create(digest2, Path::new(&Uuid::new_v4().to_string()))
            .await,
        Ok(false)
    );
    Ok(())
}

#[nativelink_test]
async fn add_file_to_cache() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    single_insert(config).await?;
    assert!(!logs_contain("ERROR"));
    assert!(!logs_contain("WARN"));
    Ok(())
}

#[nativelink_test]
async fn fails_to_evicts_because_max_entries() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        max_entries: 0,
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    single_insert(config).await?;
    assert!(!logs_contain("ERROR"));
    assert!(logs_contain(
        "Unable to evict anything from directory_cache, will exceed max entries current_items=0 max_entries=0"
    ));
    Ok(())
}

#[nativelink_test]
async fn evicts_because_max_entries() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        max_entries: 1,
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    double_insert(config).await?;
    assert!(!logs_contain("ERROR"));
    assert!(!logs_contain("WARN"));
    assert!(logs_contain(
        "Evicting cached directory digest=DigestInfo(\"0101010101010101010101010101010101010101010101010101010101010101-5\") size=0"
    ));
    Ok(())
}

#[nativelink_test]
async fn evict_with_directory_entry() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        max_entries: 1,
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    let store = MemoryStore::new(&MemorySpec::default());
    let file_digest = DigestInfo::new([3u8; 32], 5);
    assert_eq!(
        store
            .update_oneshot(file_digest, Bytes::from_static(b""))
            .await,
        Ok(())
    );
    let directory_digest = DigestInfo::new([4u8; 32], 5);
    assert_eq!(
        store
            .update_oneshot(
                directory_digest,
                Into::<Bytes>::into(
                    ProtoDirectory {
                        files: vec![],
                        directories: vec![],
                        symlinks: vec![],
                        node_properties: None
                    }
                    .encode_to_vec()
                )
            )
            .await,
        Ok(())
    );
    let encoded_directory = Into::<Bytes>::into(
        ProtoDirectory {
            files: vec![FileNode {
                name: "demo file".to_string(),
                digest: Some(file_digest.into()),
                is_executable: false,
                node_properties: None,
            }],
            directories: vec![DirectoryNode {
                name: "demo_subdir".to_string(),
                digest: Some(directory_digest.into()),
            }],
            symlinks: vec![SymlinkNode {
                name: "demo_symlink".to_string(),
                target: "demo file".to_string(),
                node_properties: None,
            }],
            node_properties: None,
        }
        .encode_to_vec(),
    );
    double_insert_with_data(config, store, encoded_directory.clone(), encoded_directory).await?;
    assert!(!logs_contain("ERROR"));
    assert!(!logs_contain("WARN"));
    assert!(logs_contain(
        "Evicting cached directory digest=DigestInfo(\"0101010101010101010101010101010101010101010101010101010101010101-5\") size=0"
    ));
    Ok(())
}
