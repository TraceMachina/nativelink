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

use nativelink_config::cas_server::PushConfig;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::asset::v1::push_server::Push;
use nativelink_proto::build::bazel::remote::asset::v1::{PushBlobRequest, PushBlobResponse};
use nativelink_service::push_server::PushServer;
use nativelink_store::store_manager::StoreManager;
use tonic::{Request, Status};

#[nativelink_test]
async fn test_push_blob() -> Result<(), Status> {
    let fs =
        PushServer::new(&PushConfig {}, &StoreManager::new()).expect("PushServer config error");
    let raw_response = fs
        .push_blob(Request::new(PushBlobRequest {
            instance_name: String::new(),
            uris: vec![],
            qualifiers: vec![],
            digest_function: 0,
            expire_at: None,
            blob_digest: None,
            references_blobs: vec![],
            references_directories: vec![],
        }))
        .await?;
    assert_eq!(raw_response.into_inner(), PushBlobResponse {});
    Ok(())
}
