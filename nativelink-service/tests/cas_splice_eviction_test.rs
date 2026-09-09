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

use std::sync::Arc;

use bytes::Bytes;
use nativelink_config::cas_server::{CasChunkingConfig, CasStoreConfig, WithInstanceName};
use nativelink_config::stores::{EvictionPolicy, MemorySpec, StoreSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_server::ContentAddressableStorage;
use nativelink_proto::build::bazel::remote::execution::v2::{
    Digest, SpliceBlobRequest, chunking_function, digest_function,
};
use nativelink_service::cas_server::CasServer;
use nativelink_service::wire_compression::RemoteCacheCompressionInstances;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::store_trait::{Store, StoreLike};
use tonic::Request;

const INSTANCE_NAME: &str = "foo_instance_name";
const CAS_MAX_BYTES: usize = 20 * 1024 * 1024;
const CHUNK_LEN: usize = 512 * 1024;
const NUM_CHUNKS: usize = 24; // 12MiB of chunks; chunks + assembled blob > cap.
const SMALL_BLOB: &[u8] = b"tiny early-build action input that queued work still references";

async fn make_capped_store_manager() -> Result<Arc<StoreManager>, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        "main_cas",
        store_factory(
            &StoreSpec::Memory(MemorySpec {
                eviction_policy: Some(EvictionPolicy {
                    max_bytes: CAS_MAX_BYTES,
                    ..Default::default()
                }),
            }),
            &store_manager,
            None,
        )
        .await?,
    )?;
    store_manager.add_store(
        "chunk_index",
        store_factory(
            &StoreSpec::Memory(MemorySpec::default()),
            &store_manager,
            None,
        )
        .await?,
    )?;
    Ok(store_manager)
}

fn make_chunking_cas_server(store_manager: &StoreManager) -> Result<CasServer, Error> {
    CasServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: CasStoreConfig {
                cas_store: "main_cas".to_string(),
                experimental_chunking: Some(CasChunkingConfig {
                    index_store: Some("chunk_index".to_string()),
                    avg_chunk_size_bytes: 0,
                    max_chunk_count: 0,
                }),
            },
        }],
        store_manager,
        &RemoteCacheCompressionInstances::default(),
    )
}

fn sha256_digest_info(data: &[u8]) -> DigestInfo {
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(data);
    hasher.finalize_digest()
}

// Regression test for a production failure: a splice stores its bytes twice
// (chunk blobs + the assembled blob) in the same size-capped CAS, so the
// assembled blob's insert-time eviction pass could push out unrelated,
// still-referenced blobs (a build-without-the-bytes client cannot re-upload
// an evicted intermediate output). With post-consumption chunk demotion,
// the eviction pass must reclaim space from the reproducible chunks
// instead.
#[nativelink_test]
async fn splice_evicts_demoted_chunks_not_unrelated_blobs()
-> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_capped_store_manager().await?;
    let cas_server = make_chunking_cas_server(&store_manager)?;
    let store: Store = store_manager.get_store("main_cas").unwrap();

    // 1. A small unrelated blob, uploaded early (oldest LRU entry).
    let small_digest = sha256_digest_info(SMALL_BLOB);
    store
        .update_oneshot(small_digest, Bytes::from_static(SMALL_BLOB))
        .await?;

    // 2. Chunks of a large blob (the CDC client's BatchUpdateBlobs phase).
    let mut blob_hasher = DigestHasherFunc::Sha256.hasher();
    let mut chunk_digests: Vec<Digest> = Vec::with_capacity(NUM_CHUNKS);
    let mut chunk_infos: Vec<DigestInfo> = Vec::with_capacity(NUM_CHUNKS);
    let mut assembled = Vec::with_capacity(NUM_CHUNKS * CHUNK_LEN);
    let mut state = 0x0005_DEEC_E66D_u64;
    for _ in 0..NUM_CHUNKS {
        let mut data = vec![0u8; CHUNK_LEN];
        for word in data.chunks_mut(8) {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let bytes = state.to_le_bytes();
            let len = word.len();
            word.copy_from_slice(&bytes[..len]);
        }
        blob_hasher.update(&data);
        assembled.extend_from_slice(&data);
        let chunk_info = sha256_digest_info(&data);
        store.update_oneshot(chunk_info, Bytes::from(data)).await?;
        chunk_digests.push(chunk_info.into());
        chunk_infos.push(chunk_info);
    }
    let blob_digest_info = blob_hasher.finalize_digest();

    // 3. Splice. Chunks + assembled blob exceed the cap, so the eviction
    // pass must run during the assembled blob's commit.
    let response = cas_server
        .splice_blob(Request::new(SpliceBlobRequest {
            instance_name: INSTANCE_NAME.to_string(),
            blob_digest: Some(blob_digest_info.into()),
            chunk_digests,
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await?
        .into_inner();
    let expected_blob_digest: Digest = blob_digest_info.into();
    assert_eq!(response.blob_digest.as_ref(), Some(&expected_blob_digest));

    // The assembled blob must be present and byte-identical.
    let stored = store.get_part_unchunked(blob_digest_info, 0, None).await?;
    assert_eq!(stored, assembled, "assembled blob corrupt after splice");

    // The unrelated small blob must have survived the eviction pass.
    assert!(
        store.has(small_digest).await?.is_some(),
        "unrelated small blob was evicted by the splice's own commit; \
         chunk demotion failed"
    );

    // Sanity: eviction pressure was real — some chunks must be gone,
    // otherwise this test is not exercising the eviction pass at all.
    let mut surviving_chunks = 0usize;
    for chunk_info in &chunk_infos {
        if store.has(*chunk_info).await?.is_some() {
            surviving_chunks += 1;
        }
    }
    assert!(
        surviving_chunks < NUM_CHUNKS,
        "expected the eviction pass to reclaim some chunks \
         (cap {CAS_MAX_BYTES} vs ~24MiB stored), got {surviving_chunks}/{NUM_CHUNKS} alive"
    );
    Ok(())
}
