// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::sync::Arc;

use nativelink_config::stores::{
    EvictionPolicy, FastSlowSpec, FilesystemSpec, MemorySpec, StoreDirection, StoreSpec,
};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::{
    Directory as ProtoDirectory, FileNode,
};
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::FilesystemStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use nativelink_worker::directory_cache::{DirectoryCache, DirectoryCacheConfig};
use prost::Message;
use tempfile::TempDir;

async fn setup_test_store() -> (
    Arc<FastSlowStore>,
    Arc<FilesystemStore>,
    TempDir,
    DigestInfo,
) {
    let temp_dir = TempDir::new().unwrap();
    let fs_root = temp_dir.path().join("filesystem_store");

    // Create FilesystemStore as the fast store
    let fs_spec = FilesystemSpec {
        content_path: fs_root.to_str().unwrap().to_string(),
        temp_path: temp_dir.path().join("temp").to_str().unwrap().to_string(),
        eviction_policy: Some(EvictionPolicy {
            max_bytes: 10 * 1024 * 1024, // 10 MB
            ..Default::default()
        }),
        ..Default::default()
    };
    let filesystem_store = FilesystemStore::new(&fs_spec).await.unwrap();

    // Wrap FilesystemStore in Store for FastSlowStore
    let fast_store = Store::new(filesystem_store.clone());

    // Create MemoryStore as the slow store
    let memory_store = Store::new(MemoryStore::new(&MemorySpec::default()));

    // Create FastSlowStore
    let fast_slow_spec = FastSlowSpec {
        fast: StoreSpec::Filesystem(fs_spec),
        fast_direction: StoreDirection::default(),
        slow: StoreSpec::Memory(MemorySpec::default()),
        slow_direction: StoreDirection::default(),
    };
    let fast_slow_store = FastSlowStore::new(&fast_slow_spec, fast_store, memory_store);

    let store = fast_slow_store.clone();

    // Create a simple directory structure
    let file_content = b"Hello, World!";
    // SHA256 hash of "Hello, World!"
    let file_digest = DigestInfo::try_new(
        "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f",
        13,
    )
    .unwrap();

    // Upload file to the store
    (*store)
        .update_oneshot(StoreKey::Digest(file_digest), file_content.to_vec().into())
        .await
        .unwrap();

    // Create Directory proto
    let directory = ProtoDirectory {
        files: vec![FileNode {
            name: "test.txt".to_string(),
            digest: Some(file_digest.into()),
            is_executable: false,
            ..Default::default()
        }],
        directories: vec![],
        symlinks: vec![],
        ..Default::default()
    };

    // Encode and upload directory
    let mut dir_data = Vec::new();
    directory.encode(&mut dir_data).unwrap();
    // Use a fixed hash for the directory
    let dir_digest = DigestInfo::try_new(
        "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        dir_data.len() as i64,
    )
    .unwrap();

    (*store)
        .update_oneshot(StoreKey::Digest(dir_digest), dir_data.into())
        .await
        .unwrap();

    (fast_slow_store, filesystem_store, temp_dir, dir_digest)
}

#[nativelink_test]
async fn directory_cache_basic_test() -> Result<(), Error> {
    let (fast_slow_store, filesystem_store, _temp_dir, dir_digest) = setup_test_store().await;

    let cache_temp_dir = TempDir::new().unwrap();
    let cache_root = cache_temp_dir.path().join("cache");

    let config = DirectoryCacheConfig {
        max_entries: 10,
        max_size_bytes: 1024 * 1024,
        cache_root,
    };

    let cache = DirectoryCache::new(config, fast_slow_store, filesystem_store).await?;

    // First access - cache miss
    let dest1 = cache_temp_dir.path().join("dest1");
    let hit = cache.get_or_create(dir_digest, &dest1).await?;
    assert!(!hit, "First access should be cache miss");
    assert!(dest1.join("test.txt").exists());

    // Verify file content
    let content = std::fs::read_to_string(dest1.join("test.txt")).unwrap();
    assert_eq!(content, "Hello, World!");

    // Second access - cache hit
    let dest2 = cache_temp_dir.path().join("dest2");
    let hit = cache.get_or_create(dir_digest, &dest2).await?;
    assert!(hit, "Second access should be cache hit");
    assert!(dest2.join("test.txt").exists());

    // Verify stats
    let stats = cache.stats().await;
    assert_eq!(stats.entries, 1);

    Ok(())
}
