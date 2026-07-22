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

use nativelink_config::stores::{ClientTlsConfig, GrpcEndpoint};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_util::tls_utils::{endpoint, endpoint_from, load_client_config};
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

#[nativelink_test]
async fn test_endpoint_from_http() -> Result<(), Error> {
    let endpoint = endpoint_from("http://localhost:50051", None)?;
    assert_eq!(endpoint.uri().scheme_str(), Some("http"));
    assert_eq!(endpoint.uri().host(), Some("localhost"));
    assert_eq!(endpoint.uri().port_u16(), Some(50051));
    Ok(())
}

#[nativelink_test]
async fn test_endpoint_from_https_with_tls() -> Result<(), Error> {
    let tls_config = load_client_config(&Some(ClientTlsConfig {
        use_native_roots: Some(true),
        ca_file: None,
        cert_file: None,
        key_file: None,
    }))?;
    let endpoint = endpoint_from("https://example.com", tls_config)?;
    assert_eq!(endpoint.uri().scheme_str(), Some("https"));
    assert_eq!(endpoint.uri().host(), Some("example.com"));
    Ok(())
}

#[nativelink_test]
async fn test_endpoint_from_grpcs_with_tls() -> Result<(), Error> {
    let tls_config = load_client_config(&Some(ClientTlsConfig {
        use_native_roots: Some(true),
        ca_file: None,
        cert_file: None,
        key_file: None,
    }))?;
    let endpoint = endpoint_from("grpcs://example.com", tls_config)?;
    assert_eq!(endpoint.uri().scheme_str(), Some("https"));
    assert_eq!(endpoint.uri().host(), Some("example.com"));
    Ok(())
}

#[nativelink_test]
async fn test_endpoint_from_https_without_tls() -> Result<(), Error> {
    let result = endpoint_from("https://example.com", None);
    assert!(matches!(
        result,
        Err(e) if e.to_string().contains("is https or grpcs, but no TLS configuration was provided")
    ));
    Ok(())
}

#[nativelink_test]
async fn test_endpoint_from_http_with_tls() -> Result<(), Error> {
    let tls_config = load_client_config(&Some(ClientTlsConfig {
        use_native_roots: Some(true),
        ca_file: None,
        cert_file: None,
        key_file: None,
    }))?;
    let result = endpoint_from("http://example.com:8080", tls_config);
    assert!(matches!(
        result,
        Err(e) if e.to_string().contains("but the scheme is not https or grpcs")
    ));
    Ok(())
}

#[nativelink_test]
async fn test_endpoint_from_invalid_uri() -> Result<(), Error> {
    let result = endpoint_from("not a valid uri", None);
    assert!(matches!(
        result,
        Err(e) if e.to_string().contains("Unable to parse endpoint")
    ));
    Ok(())
}

#[nativelink_test]
async fn test_endpoint_from_missing_authority() -> Result<(), Error> {
    let tls_config = load_client_config(&Some(ClientTlsConfig {
        use_native_roots: Some(true),
        ca_file: None,
        cert_file: None,
        key_file: None,
    }))?;
    let result = endpoint_from("/path/no/authority", tls_config);
    assert!(matches!(
        result,
        Err(e) if e.to_string().contains("Unable to determine authority of endpoint")
    ));
    Ok(())
}

#[nativelink_test]
async fn test_endpoint_with_http2_window_tuning() -> Result<(), Error> {
    let config = GrpcEndpoint {
        address: "grpc://localhost:50051".to_string(),
        tls_config: None,
        concurrency_limit: None,
        connect_timeout_s: 0,
        tcp_keepalive_s: 0,
        http2_keepalive_interval_s: 0,
        http2_keepalive_timeout_s: 0,
        experimental_http2_initial_stream_window_size: Some(8 * 1024 * 1024),
        experimental_http2_initial_connection_window_size: Some(32 * 1024 * 1024),
        experimental_http2_adaptive_window: Some(true),
        experimental_http2_max_frame_size: Some(1024 * 1024),
    };
    drop(endpoint(&config)?);
    Ok(())
}

#[nativelink_test]
async fn test_endpoint_without_http2_window_tuning() -> Result<(), Error> {
    let config = GrpcEndpoint {
        address: "grpc://localhost:50051".to_string(),
        tls_config: None,
        concurrency_limit: None,
        connect_timeout_s: 0,
        tcp_keepalive_s: 0,
        http2_keepalive_interval_s: 0,
        http2_keepalive_timeout_s: 0,
        experimental_http2_initial_stream_window_size: None,
        experimental_http2_initial_connection_window_size: None,
        experimental_http2_adaptive_window: None,
        experimental_http2_max_frame_size: None,
    };
    drop(endpoint(&config)?);
    Ok(())
}

fn tuned_endpoint_config() -> GrpcEndpoint {
    GrpcEndpoint {
        address: "grpc://localhost:50051".to_string(),
        tls_config: None,
        concurrency_limit: None,
        connect_timeout_s: 0,
        tcp_keepalive_s: 0,
        http2_keepalive_interval_s: 0,
        http2_keepalive_timeout_s: 0,
        experimental_http2_initial_stream_window_size: None,
        experimental_http2_initial_connection_window_size: None,
        experimental_http2_adaptive_window: None,
        experimental_http2_max_frame_size: None,
    }
}

#[nativelink_test]
async fn test_endpoint_rejects_invalid_http2_values() -> Result<(), Error> {
    let mut config = tuned_endpoint_config();
    config.experimental_http2_initial_stream_window_size = Some(0);
    assert!(
        endpoint(&config)
            .expect_err("zero stream windows must be rejected")
            .to_string()
            .contains("between 1 and")
    );

    config = tuned_endpoint_config();
    config.experimental_http2_initial_connection_window_size = Some(0);
    assert!(
        endpoint(&config)
            .expect_err("zero connection windows must be rejected")
            .to_string()
            .contains("between 1 and")
    );

    config = tuned_endpoint_config();
    config.experimental_http2_initial_stream_window_size = Some(0x8000_0000);
    assert!(
        endpoint(&config)
            .expect_err("oversized stream windows must be rejected")
            .to_string()
            .contains("initial_stream_window_size")
    );

    config = tuned_endpoint_config();
    config.experimental_http2_initial_connection_window_size = Some(0x8000_0000);
    assert!(
        endpoint(&config)
            .expect_err("oversized connection windows must be rejected")
            .to_string()
            .contains("initial_connection_window_size")
    );

    config = tuned_endpoint_config();
    config.experimental_http2_max_frame_size = Some(16_383);
    assert!(endpoint(&config).is_err());

    config = tuned_endpoint_config();
    config.experimental_http2_max_frame_size = Some(16_777_216);
    assert!(endpoint(&config).is_err());
    Ok(())
}

#[nativelink_test]
async fn test_endpoint_adaptive_window_overrides_explicit_windows() -> Result<(), Error> {
    let mut config = tuned_endpoint_config();
    config.experimental_http2_adaptive_window = Some(true);
    config.experimental_http2_initial_stream_window_size = Some(8 * 1024 * 1024);
    config.experimental_http2_initial_connection_window_size = Some(32 * 1024 * 1024);
    drop(endpoint(&config)?);
    Ok(())
}

#[nativelink_test]
async fn test_endpoint_accepts_http2_setting_boundaries() -> Result<(), Error> {
    let mut config = tuned_endpoint_config();
    config.experimental_http2_initial_stream_window_size = Some(1);
    config.experimental_http2_initial_connection_window_size = Some(0x7fff_ffff);
    config.experimental_http2_max_frame_size = Some(16_384);
    drop(endpoint(&config)?);

    config.experimental_http2_max_frame_size = Some(16_777_215);
    drop(endpoint(&config)?);
    Ok(())
}
