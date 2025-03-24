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

use nativelink_config::cas_server::FetchConfig;
use nativelink_error::{make_err, Code};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::asset::v1::fetch_server::Fetch;
use nativelink_proto::build::bazel::remote::asset::v1::{FetchBlobRequest, FetchBlobResponse};
use nativelink_proto::build::bazel::remote::execution::v2::digest_function;
use nativelink_service::fetch_server::FetchServer;
use nativelink_store::store_manager::StoreManager;
use tonic::Request;

#[nativelink_test]
async fn test_fetch_blob() {
    let fs = FetchServer::new(&FetchConfig {}, &StoreManager::new()).unwrap();
    let raw_response = fs
        .fetch_blob(Request::new(FetchBlobRequest {
            instance_name: String::new(),
            timeout: None,
            oldest_content_accepted: None,
            uris: vec![],
            qualifiers: vec![],
            digest_function: 0,
        }))
        .await
        .unwrap();
    assert_eq!(
        raw_response.into_inner(),
        FetchBlobResponse {
            status: Some(make_err!(Code::NotFound, "No item found").into()),
            uri: String::new(),
            qualifiers: vec![],
            expires_at: None,
            blob_digest: None,
            digest_function: digest_function::Value::Sha256.into()
        }
    );
}
