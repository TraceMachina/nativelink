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

//! Verifies that AWS credential discovery can use link-local HTTP endpoints
//! without allowing unencrypted HTTP for S3 requests.

use aws_smithy_runtime_api::client::http::HttpConnector;
use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
use nativelink_config::stores::CommonObjectSpec;
use nativelink_macro::nativelink_test;
use nativelink_store::common_s3_utils::TlsClient;
use nativelink_util::spawn;
use nativelink_util::task::JoinHandleDropGuard;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn start_http_server() -> (String, JoinHandleDropGuard<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = spawn!("credential HTTP test server", async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 1024];
        let _bytes_read = socket.read(&mut request).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });
    (format!("http://{address}/credentials"), server)
}

#[nativelink_test]
async fn credential_client_allows_http() {
    let common = CommonObjectSpec::default();
    assert!(!common.insecure_allow_http);

    let (credential_url, credential_server) = start_http_server().await;
    let credential_result = TlsClient::new_for_credentials(&common)
        .call(HttpRequest::get(credential_url).unwrap())
        .await;
    assert!(
        credential_result.is_ok(),
        "credential client should allow HTTP: {credential_result:?}"
    );
    credential_server.await.unwrap();
}

#[nativelink_test]
async fn s3_client_rejects_http_by_default() {
    let common = CommonObjectSpec::default();
    assert!(!common.insecure_allow_http);

    let (s3_url, s3_server) = start_http_server().await;
    let s3_result = TlsClient::new(&common)
        .call(HttpRequest::get(s3_url).unwrap())
        .await;
    drop(s3_server);
    assert!(
        s3_result.is_err(),
        "S3 client should reject HTTP: {s3_result:?}"
    );
}
