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

use nativelink_config::stores::ClientTlsConfig;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_util::tls_utils::load_client_config;
use tempfile::NamedTempFile;

#[nativelink_test]
async fn test_load_client_config_none() -> Result<(), Error> {
    let config = load_client_config(&None)?;
    assert!(config.is_none());
    Ok(())
}

#[nativelink_test]
async fn test_load_client_config_native_roots() -> Result<(), Error> {
    let config = load_client_config(&Some(ClientTlsConfig {
        use_native_roots: Some(true),
        ca_file: None,
        cert_file: None,
        key_file: None,
    }))?;
    assert!(config.is_some());
    Ok(())
}

#[nativelink_test]
async fn test_load_client_config_missing_ca() -> Result<(), Error> {
    let result = load_client_config(&Some(ClientTlsConfig {
        use_native_roots: None,
        ca_file: None,
        cert_file: None,
        key_file: None,
    }));
    assert!(matches!(
        result,
        Err(e) if e.to_string().contains("CA certificate must be provided")
    ));
    Ok(())
}

#[nativelink_test]
async fn test_load_client_config_cert_without_key() -> Result<(), Error> {
    let temp_file = NamedTempFile::new()?;
    let result = load_client_config(&Some(ClientTlsConfig {
        use_native_roots: None,
        ca_file: Some(temp_file.path().to_str().unwrap().to_string()),
        cert_file: Some("tls.crt".to_string()),
        key_file: None,
    }));
    assert!(matches!(
        result,
        Err(e) if e.to_string().contains("Client certificate specified, but no key")
    ));
    Ok(())
}

#[nativelink_test]
async fn test_load_client_config_key_without_cert() -> Result<(), Error> {
    let temp_file = NamedTempFile::new()?;
    let result = load_client_config(&Some(ClientTlsConfig {
        use_native_roots: None,
        ca_file: Some(temp_file.path().to_str().unwrap().to_string()),
        cert_file: None,
        key_file: Some("tls.key".to_string()),
    }));
    assert!(matches!(
        result,
        Err(e) if e.to_string().contains("Client key specified, but no certificate")
    ));
    Ok(())
}

#[nativelink_test]
async fn test_load_client_config_with_cert_files() -> Result<(), Error> {
    let temp_file = NamedTempFile::new()?;
    let config = load_client_config(&Some(ClientTlsConfig {
        use_native_roots: None,
        ca_file: Some(temp_file.path().to_str().unwrap().to_string()),
        cert_file: Some(temp_file.path().to_str().unwrap().to_string()),
        key_file: Some(temp_file.path().to_str().unwrap().to_string()),
    }))?;
    assert!(config.is_some());
    Ok(())
}
