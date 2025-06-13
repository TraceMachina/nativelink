// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use nativelink_config::cas_server::{PushConfig, WithInstanceName};
use nativelink_config::stores::{MemorySpec, StoreSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::asset::v1::push_server::Push;
use nativelink_proto::build::bazel::remote::asset::v1::{
    PushBlobRequest, PushBlobResponse, Qualifier,
};
use nativelink_proto::build::bazel::remote::execution::v2::Digest;
use nativelink_service::push_server::PushServer;
use nativelink_service::remote_asset_proto::RemoteAssetArtifact;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::StoreLike;
use prost::Message;
use sha2::{Digest as Sha2Digest, Sha256};
use tonic::{Request, Status};

async fn make_store_manager() -> Result<Arc<StoreManager>, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        "test_push_store",
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
async fn test_push_blob() -> Result<(), Status> {
    let store_manager = make_store_manager().await?;
    let instance_name = "foo_instance_name".to_string();
    let ps = PushServer::new(
        &[WithInstanceName {
            instance_name: instance_name.clone(),
            config: PushConfig {
                push_store: String::from("test_push_store"),
                read_only: false,
            },
        }],
        &store_manager,
    )
    .expect("PushServer config error");
    let test_hash = Sha256::new();
    let raw_response = ps
        .push_blob(Request::new(PushBlobRequest {
            instance_name,
            uris: vec!["http://1234".to_owned()],
            qualifiers: vec![Qualifier {
                name: "resource_type".to_owned(),
                value: "test".to_owned(),
            }],
            digest_function: 0,
            expire_at: None,
            blob_digest: Some(Digest {
                hash: hex::encode(test_hash.finalize()),
                size_bytes: 4,
            }),
            references_blobs: vec![],
            references_directories: vec![],
        }))
        .await?;
    assert_eq!(raw_response.into_inner(), PushBlobResponse {});

    let push_store = store_manager.get_store("test_push_store").unwrap();

    let digest = DigestInfo::try_new(
        "95eb05bf25e3e14275625ecb8d99bc8a82b4b2afec7334dd696dc19107eb9b05",
        28,
    )?;
    let raw_data = push_store.get_part_unchunked(digest, 0, None).await?;

    let decoded_asset_artifact = RemoteAssetArtifact::decode(raw_data).unwrap();
    assert_eq!(
        decoded_asset_artifact,
        RemoteAssetArtifact::new(
            "http://1234".to_owned(),
            vec![Qualifier {
                name: "resource_type".to_owned(),
                value: "test".to_owned(),
            }],
            Digest {
                hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .to_string(),
                size_bytes: 4,
            },
            None,
            0,
        )
    );
    Ok(())
}
