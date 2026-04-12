use core::time::Duration;

use nativelink_config::stores::{GrpcEndpoint, GrpcSpec, Retry, StoreType};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::{
    FindMissingBlobsRequest, digest_function,
};
use nativelink_store::grpc_store::GrpcStore;
use tokio::time::timeout;
use tonic::Request;

fn make_test_endpoint() -> GrpcEndpoint {
    GrpcEndpoint {
        address: "http://foobar".into(),
        tls_config: None,
        concurrency_limit: None,
        connect_timeout_s: 0,
        tcp_keepalive_s: 0,
        http2_keepalive_interval_s: 0,
        http2_keepalive_timeout_s: 0,
        tcp_nodelay: true,
        use_http3: false,
    }
}

fn make_test_spec() -> GrpcSpec {
    GrpcSpec {
        instance_name: String::new(),
        endpoints: vec![make_test_endpoint()],
        store_type: StoreType::Cas,
        retry: Retry::default(),
        max_concurrent_requests: 0,
        connections_per_endpoint: 0,
        rpc_timeout_s: 1,
        batch_update_threshold_bytes: 0,
        max_concurrent_batch_rpcs: 8,
        parallel_chunk_read_threshold: 0,
        parallel_chunk_count: 0,
        dual_transport: false,
        zstd_compression: false,
    }
}

#[nativelink_test]
async fn fast_find_missing_blobs() -> Result<(), Error> {
    let spec = make_test_spec();
    let store = GrpcStore::new(&spec).await?;
    let request = Request::new(FindMissingBlobsRequest {
        instance_name: String::new(),
        blob_digests: vec![],
        digest_function: digest_function::Value::Sha256.into(),
    });
    let res = timeout(Duration::from_secs(1), async move {
        store.find_missing_blobs(request).await
    })
    .await??;
    let inner_res = res.into_inner();
    assert_eq!(inner_res.missing_blob_digests.len(), 0);
    Ok(())
}

/// Verify that GrpcStore can be constructed with zstd_compression enabled.
/// The actual compression negotiation requires a real server, but we verify
/// the store builds without error and that find_missing_blobs still works
/// (the endpoint is fake, so the RPC completes immediately with empty results).
#[nativelink_test]
async fn grpc_store_with_zstd_compression_creates_successfully() -> Result<(), Error> {
    let mut spec = make_test_spec();
    spec.zstd_compression = true;
    let store = GrpcStore::new(&spec).await?;
    // Exercise the client creation path by issuing a find_missing_blobs.
    let request = Request::new(FindMissingBlobsRequest {
        instance_name: String::new(),
        blob_digests: vec![],
        digest_function: digest_function::Value::Sha256.into(),
    });
    let res = timeout(Duration::from_secs(1), async move {
        store.find_missing_blobs(request).await
    })
    .await??;
    assert_eq!(res.into_inner().missing_blob_digests.len(), 0);
    Ok(())
}

/// Verify that zstd_compression=false (default) also works as before.
#[nativelink_test]
async fn grpc_store_without_zstd_compression() -> Result<(), Error> {
    let spec = make_test_spec();
    assert!(!spec.zstd_compression, "default should be false");
    let store = GrpcStore::new(&spec).await?;
    let request = Request::new(FindMissingBlobsRequest {
        instance_name: String::new(),
        blob_digests: vec![],
        digest_function: digest_function::Value::Sha256.into(),
    });
    let res = timeout(Duration::from_secs(1), async move {
        store.find_missing_blobs(request).await
    })
    .await??;
    assert_eq!(res.into_inner().missing_blob_digests.len(), 0);
    Ok(())
}
