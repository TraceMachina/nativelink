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

//! Verifies that `TlsClient` reports a configuration error instead of
//! panicking when no CA root certificates can be loaded.
//!
//! Only Linux reads `SSL_CERT_FILE`/`SSL_CERT_DIR` for the platform
//! verifier, and this binary is kept separate from the other `TlsClient`
//! tests because those environment variables are process-wide.

#![cfg(target_os = "linux")]

use nativelink_config::stores::CommonObjectSpec;
use nativelink_error::Code;
use nativelink_macro::nativelink_test;
use nativelink_store::common_s3_utils::TlsClient;

#[nativelink_test]
async fn tls_client_without_ca_roots_returns_error() {
    let empty_dir = tempfile::tempdir().unwrap();
    let empty_file = empty_dir.path().join("empty.pem");
    std::fs::write(&empty_file, b"").unwrap();
    // SAFETY: this test binary has no other tests reading the environment.
    unsafe {
        std::env::set_var("SSL_CERT_FILE", &empty_file);
        std::env::set_var("SSL_CERT_DIR", empty_dir.path());
    }

    let common = CommonObjectSpec::default();
    for result in [
        TlsClient::new(&common),
        TlsClient::new_for_credentials(&common),
    ] {
        let err = result.expect_err("TlsClient must not build without CA roots");
        assert_eq!(err.code, Code::InvalidArgument, "{err:?}");
        assert!(
            err.to_string().contains("SSL_CERT_FILE"),
            "error should tell the operator how to supply roots: {err:?}"
        );
    }
}
