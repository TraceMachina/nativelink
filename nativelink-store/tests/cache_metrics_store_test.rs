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

use std::sync::Arc;

use nativelink_config::stores::{
    CacheMetricsSpec, FastSlowSpec, FilesystemSpec, MemorySpec, NoopSpec, RedisSpec, RefSpec,
    SizePartitioningSpec, StoreDirection, StoreSpec, VerifySpec,
};
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_redis_tester::{
    add_lua_script, add_to_response, fake_redis_stream, make_fake_redis_with_responses,
};
use nativelink_store::cache_metrics_store::CacheMetricsStore;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::redis_store::LUA_VERSION_SET_SCRIPT;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::{StoreKey, StoreLike};
use pretty_assertions::assert_eq;

const VALID_HASH: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
const MISSING_HASH: &str = "0123456789abcdef000000000000000000020000000000000123456789abcdef";

#[nativelink_test]
async fn cache_metrics_store_is_only_constructed_when_configured() -> Result<(), Error> {
    let store_manager = Arc::new(StoreManager::new());

    let plain_store = store_factory(
        &StoreSpec::Memory(MemorySpec::default()),
        &store_manager,
        None,
    )
    .await?;
    assert!(
        plain_store
            .downcast_ref::<CacheMetricsStore>(None)
            .is_none(),
        "plain memory store should not be wrapped with cache metrics"
    );

    let wrapped_store = store_factory(
        &StoreSpec::CacheMetrics(Box::new(CacheMetricsSpec {
            cache_type: "cas".to_string(),
            backend: StoreSpec::Memory(MemorySpec::default()),
        })),
        &store_manager,
        None,
    )
    .await?;
    assert!(
        wrapped_store
            .downcast_ref::<CacheMetricsStore>(None)
            .is_some(),
        "cache_metrics config should construct the metrics wrapper"
    );

    Ok(())
}

#[nativelink_test]
async fn cache_metrics_store_preserves_backend_store_semantics() -> Result<(), Error> {
    let store_manager = Arc::new(StoreManager::new());
    let store = store_factory(
        &StoreSpec::CacheMetrics(Box::new(CacheMetricsSpec {
            cache_type: "cas".to_string(),
            backend: StoreSpec::Memory(MemorySpec::default()),
        })),
        &store_manager,
        None,
    )
    .await?;

    let value = "cache metrics payload";
    let digest = DigestInfo::try_new(VALID_HASH, value.len())?;
    store.update_oneshot(digest, value.into()).await?;

    assert_eq!(store.has(digest).await?, Some(value.len() as u64));
    assert_eq!(
        store.get_part_unchunked(digest, 0, None).await?,
        value.as_bytes()
    );

    let missing_digest = DigestInfo::try_new(MISSING_HASH, value.len())?;
    let missing = store.get_part_unchunked(missing_digest, 0, None).await;
    assert_eq!(missing.unwrap_err().code, Code::NotFound);

    assert!(
        store
            .as_store_driver_pin()
            .remove(StoreKey::from(digest))
            .await?
    );
    assert_eq!(store.has(digest).await?, None);
    assert!(
        !store
            .as_store_driver_pin()
            .remove(StoreKey::from(digest))
            .await?
    );

    Ok(())
}

fn metrics_backend(name: &str) -> StoreSpec {
    StoreSpec::CacheMetrics(Box::new(CacheMetricsSpec {
        cache_type: "cas".into(),
        backend: StoreSpec::RefStore(RefSpec { name: name.into() }),
    }))
}

fn fast_slow(fast: StoreSpec, slow: StoreSpec) -> StoreSpec {
    StoreSpec::FastSlow(Box::new(FastSlowSpec {
        fast,
        slow,
        fast_direction: StoreDirection::Both,
        slow_direction: StoreDirection::Both,
        bypass_dedup_threshold_bytes: 0,
    }))
}

async fn repair_truncated_cache(corrupt_tier: &str, truncated: &'static str) -> Result<(), Error> {
    let payload = "v".repeat(78);
    let digest = DigestInfo::try_new(VALID_HASH, payload.len())?;
    // Use the real RedisStore with a local protocol fixture for the durable
    // copy. Its default remove() must retain that copy during cache repair.
    let mut responses = fake_redis_stream();
    add_lua_script(
        &mut responses,
        LUA_VERSION_SET_SCRIPT,
        "d89e3573a1f9689c22115e8a41bd332d0c7c2643",
    );
    add_to_response(
        &mut responses,
        redis::cmd("STRLEN").arg(digest.to_string()),
        // RedisStore sends STRLEN and EXISTS together as a pipeline.
        vec![redis::Value::Int(78), redis::Value::Int(1)],
    );
    add_to_response(
        &mut responses,
        redis::cmd("EXISTS").arg(digest.to_string()),
        vec![redis::Value::Int(1)],
    );
    for end in [77, 65535] {
        add_to_response(
            &mut responses,
            redis::cmd("GETRANGE")
                .arg(digest.to_string())
                .arg(0)
                .arg(end),
            vec![redis::Value::BulkString(payload.as_bytes().to_vec())],
        );
    }
    let port = make_fake_redis_with_responses(responses).await;
    let durable_spec = StoreSpec::RedisStore(RedisSpec {
        addresses: vec![format!("redis://127.0.0.1:{port}/")],
        command_timeout_ms: 1000,
        ..Default::default()
    });
    let directory = tempfile::tempdir()?;
    let manager = Arc::new(StoreManager::new());
    let memory = store_factory(&StoreSpec::Memory(MemorySpec::default()), &manager, None).await?;
    let filesystem = store_factory(
        &StoreSpec::Filesystem(FilesystemSpec {
            content_path: directory
                .path()
                .join("content")
                .to_string_lossy()
                .into_owned(),
            temp_path: directory.path().join("temp").to_string_lossy().into_owned(),
            block_size: 1,
            ..Default::default()
        }),
        &manager,
        None,
    )
    .await?;
    let durable = store_factory(&durable_spec, &manager, None).await?;
    manager.add_store("memory", memory.clone())?;
    manager.add_store("filesystem", filesystem.clone())?;
    manager.add_store("durable", durable.clone())?;

    let backend = fast_slow(
        StoreSpec::SizePartitioning(Box::new(SizePartitioningSpec {
            size: 1024,
            lower_store: metrics_backend("memory"),
            upper_store: StoreSpec::Noop(NoopSpec {}),
        })),
        fast_slow(
            metrics_backend("filesystem"),
            StoreSpec::RefStore(RefSpec {
                name: "durable".into(),
            }),
        ),
    );
    let store = store_factory(
        &StoreSpec::Verify(Box::new(VerifySpec {
            verify_size: true,
            verify_hash: false,
            backend,
        })),
        &manager,
        None,
    )
    .await?;
    store.clone().into_inner().post_init().await?;
    manager
        .get_store(corrupt_tier)
        .unwrap()
        .update_oneshot(digest, truncated.into())
        .await?;

    let err = tokio::time::timeout(
        core::time::Duration::from_secs(5),
        store.get_part_unchunked(digest, 0, None),
    )
    .await
    .expect("the corrupt read must finish")
    .unwrap_err();
    assert_eq!(err.code, Code::DataLoss, "{err:?}");
    assert_eq!(
        memory.has(digest).await?,
        None,
        "bad memory copy must be removed"
    );
    assert_eq!(
        filesystem.has(digest).await?,
        None,
        "bad filesystem copy must be removed"
    );
    assert_eq!(
        durable.get_part_unchunked(digest, 0, None).await?,
        payload.as_bytes()
    );
    assert!(
        !durable
            .as_store_driver_pin()
            .remove(StoreKey::from(digest))
            .await?
    );

    let recovered = tokio::time::timeout(
        core::time::Duration::from_secs(5),
        store.get_part_unchunked(digest, 0, None),
    )
    .await
    .expect("the repair read must finish")?;
    assert_eq!(recovered, payload.as_bytes());
    assert_eq!(
        memory.get_part_unchunked(digest, 0, None).await?,
        payload.as_bytes()
    );
    assert_eq!(
        filesystem.get_part_unchunked(digest, 0, None).await?,
        payload.as_bytes()
    );
    Ok(())
}

#[nativelink_test]
async fn verification_repairs_the_metrics_wrapped_memory_tier() -> Result<(), Error> {
    for truncated in ["", "short"] {
        repair_truncated_cache("memory", truncated).await?;
    }
    Ok(())
}

#[nativelink_test]
async fn verification_repairs_the_metrics_wrapped_filesystem_tier() -> Result<(), Error> {
    for truncated in ["", "short"] {
        repair_truncated_cache("filesystem", truncated).await?;
    }
    Ok(())
}
