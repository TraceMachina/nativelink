// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::time::Duration;

use nativelink_config::stores::{ClientTlsConfig, GrpcEndpoint};
use nativelink_error::{Code, Error, make_err, make_input_err};
use tonic::transport::Uri;
use tracing::{info, warn};

pub fn load_client_config(
    config: &Option<ClientTlsConfig>,
) -> Result<Option<tonic::transport::ClientTlsConfig>, Error> {
    let Some(config) = config else {
        return Ok(None);
    };

    if config.use_native_roots == Some(true) {
        if config.ca_file.is_some() {
            warn!("Native root certificates are being used, all certificate files will be ignored");
        }
        return Ok(Some(
            tonic::transport::ClientTlsConfig::new().with_native_roots(),
        ));
    }

    let Some(ca_file) = &config.ca_file else {
        return Err(make_err!(
            Code::Internal,
            "CA certificate must be provided if not using native root certificates"
        ));
    };

    let read_config = tonic::transport::ClientTlsConfig::new().ca_certificate(
        tonic::transport::Certificate::from_pem(std::fs::read_to_string(ca_file)?),
    );
    let config = if let Some(client_certificate) = &config.cert_file {
        let Some(client_key) = &config.key_file else {
            return Err(make_err!(
                Code::Internal,
                "Client certificate specified, but no key"
            ));
        };
        read_config.identity(tonic::transport::Identity::from_pem(
            std::fs::read_to_string(client_certificate)?,
            std::fs::read_to_string(client_key)?,
        ))
    } else {
        if config.key_file.is_some() {
            return Err(make_err!(
                Code::Internal,
                "Client key specified, but no certificate"
            ));
        }
        read_config
    };

    Ok(Some(config))
}

pub fn endpoint_from(
    endpoint: &str,
    tls_config: Option<tonic::transport::ClientTlsConfig>,
) -> Result<tonic::transport::Endpoint, Error> {
    let endpoint = Uri::try_from(endpoint).map_err(|e| {
        Error::from_std_err(Code::Internal, &e)
            .append(format!("Unable to parse endpoint {endpoint}"))
    })?;

    // Tonic uses the TLS configuration if the scheme is "https", so replace
    // grpcs with https.
    let endpoint = if endpoint.scheme_str() == Some("grpcs") {
        let mut parts = endpoint.into_parts();
        parts.scheme = Some("https".parse().map_err(|e| {
            Error::from_std_err(Code::Internal, &e).append("https is an invalid scheme apparently?")
        })?);
        parts.try_into().map_err(|e| {
            Error::from_std_err(Code::Internal, &e).append("Error changing Uri from grpcs to https")
        })?
    } else {
        endpoint
    };

    let endpoint_transport = if let Some(tls_config) = tls_config {
        let Some(authority) = endpoint.authority() else {
            return Err(make_input_err!(
                "Unable to determine authority of endpoint: {endpoint}"
            ));
        };
        if endpoint.scheme_str() != Some("https") {
            return Err(make_input_err!(
                "You have set TLS configuration on {endpoint}, but the scheme is not https or grpcs"
            ));
        }
        let tls_config = tls_config.domain_name(authority.host());
        tonic::transport::Endpoint::from(endpoint)
            .tls_config(tls_config)
            .map_err(|e| {
                Error::from_std_err(Code::InvalidArgument, &e).append("Setting mTLS configuration")
            })?
    } else {
        if endpoint.scheme_str() == Some("https") {
            return Err(make_input_err!(
                "The scheme of {endpoint} is https or grpcs, but no TLS configuration was provided"
            ));
        }
        tonic::transport::Endpoint::from(endpoint)
    };

    Ok(endpoint_transport)
}

pub fn endpoint(endpoint_config: &GrpcEndpoint) -> Result<tonic::transport::Endpoint, Error> {
    const MAX_HTTP2_WINDOW_SIZE: u32 = 0x7fff_ffff;
    const MIN_HTTP2_FRAME_SIZE: u32 = 16_384;
    const MAX_HTTP2_FRAME_SIZE: u32 = 16_777_215;

    if endpoint_config
        .experimental_http2_initial_stream_window_size
        .is_some_and(|size| size == 0 || size > MAX_HTTP2_WINDOW_SIZE)
    {
        return Err(make_input_err!(
            "experimental_http2_initial_stream_window_size must be between 1 and {MAX_HTTP2_WINDOW_SIZE}"
        ));
    }
    if endpoint_config
        .experimental_http2_initial_connection_window_size
        .is_some_and(|size| size == 0 || size > MAX_HTTP2_WINDOW_SIZE)
    {
        return Err(make_input_err!(
            "experimental_http2_initial_connection_window_size must be between 1 and {MAX_HTTP2_WINDOW_SIZE}"
        ));
    }
    if endpoint_config
        .experimental_http2_max_frame_size
        .is_some_and(|size| !(MIN_HTTP2_FRAME_SIZE..=MAX_HTTP2_FRAME_SIZE).contains(&size))
    {
        return Err(make_input_err!(
            "experimental_http2_max_frame_size must be between {MIN_HTTP2_FRAME_SIZE} and {MAX_HTTP2_FRAME_SIZE}"
        ));
    }
    let endpoint = endpoint_from(
        &endpoint_config.address,
        load_client_config(&endpoint_config.tls_config)?,
    )?;

    let connect_timeout = if endpoint_config.connect_timeout_s > 0 {
        Duration::from_secs(endpoint_config.connect_timeout_s)
    } else {
        Duration::from_secs(30)
    };
    let tcp_keepalive = if endpoint_config.tcp_keepalive_s > 0 {
        Duration::from_secs(endpoint_config.tcp_keepalive_s)
    } else {
        Duration::from_secs(30)
    };
    let http2_keepalive_interval = if endpoint_config.http2_keepalive_interval_s > 0 {
        Duration::from_secs(endpoint_config.http2_keepalive_interval_s)
    } else {
        Duration::from_secs(30)
    };
    let http2_keepalive_timeout = if endpoint_config.http2_keepalive_timeout_s > 0 {
        Duration::from_secs(endpoint_config.http2_keepalive_timeout_s)
    } else {
        Duration::from_secs(20)
    };

    info!(
        address = %endpoint_config.address,
        concurrency_limit = ?endpoint_config.concurrency_limit,
        connect_timeout_s = connect_timeout.as_secs(),
        tcp_keepalive_s = tcp_keepalive.as_secs(),
        http2_keepalive_interval_s = http2_keepalive_interval.as_secs(),
        http2_keepalive_timeout_s = http2_keepalive_timeout.as_secs(),
        http2_initial_stream_window_size = ?endpoint_config.experimental_http2_initial_stream_window_size,
        http2_initial_connection_window_size = ?endpoint_config.experimental_http2_initial_connection_window_size,
        http2_adaptive_window = ?endpoint_config.experimental_http2_adaptive_window,
        http2_max_frame_size = ?endpoint_config.experimental_http2_max_frame_size,
        "tls_utils::endpoint: creating gRPC endpoint with keepalive",
    );

    let mut endpoint = endpoint
        .connect_timeout(connect_timeout)
        .tcp_keepalive(Some(tcp_keepalive))
        .http2_keep_alive_interval(http2_keepalive_interval)
        .keep_alive_timeout(http2_keepalive_timeout)
        .keep_alive_while_idle(true);

    if let Some(concurrency_limit) = endpoint_config.concurrency_limit {
        endpoint = endpoint.concurrency_limit(concurrency_limit);
    }
    // HTTP/2 flow-control tuning: these are client receive windows and
    // primarily affect downloads from the upstream endpoint. Upload receive
    // windows are controlled by the receiving server.
    if endpoint_config.experimental_http2_adaptive_window != Some(true) {
        if let Some(window_size) = endpoint_config.experimental_http2_initial_stream_window_size {
            endpoint = endpoint.initial_stream_window_size(window_size);
        }
        if let Some(window_size) = endpoint_config.experimental_http2_initial_connection_window_size
        {
            endpoint = endpoint.initial_connection_window_size(window_size);
        }
    }
    if let Some(adaptive_window) = endpoint_config.experimental_http2_adaptive_window {
        endpoint = endpoint.http2_adaptive_window(adaptive_window);
    }
    if let Some(frame_size) = endpoint_config.experimental_http2_max_frame_size {
        endpoint = endpoint.max_frame_size(frame_size);
    }

    Ok(endpoint)
}
