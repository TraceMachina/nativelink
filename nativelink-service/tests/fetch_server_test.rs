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

use nativelink_config::cas_server::{FetchConfig, WithInstanceName};
use nativelink_config::stores::{MemorySpec, StoreSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::asset::v1::fetch_server::Fetch;
use nativelink_proto::build::bazel::remote::asset::v1::{
    FetchBlobRequest, FetchBlobResponse, Qualifier,
};
use nativelink_proto::build::bazel::remote::execution::v2::Digest;
use nativelink_proto::google::rpc::Status as GoogleStatus;
use nativelink_service::fetch_server::FetchServer;
use nativelink_service::remote_asset_proto::RemoteAssetArtifact;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::store_trait::StoreLike;
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
    );
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
