// Copyright 2024 The Native Link Authors. All rights reserved.
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
use nativelink_error::{make_err, make_input_err, Code, Error};
use tonic::transport::Uri;

pub fn load_client_config(
    config: &Option<ClientTlsConfig>,
) -> Result<Option<tonic::transport::ClientTlsConfig>, Error> {
    let Some(config) = config else {
        return Ok(None);
    };

    let read_config = tonic::transport::ClientTlsConfig::new().ca_certificate(tonic::transport::Certificate::from_pem(
        std::fs::read_to_string(&config.ca_file)?,
    ));
    let config = if let Some(client_certificate) = &config.cert_file {
        let Some(client_key) = &config.key_file else {
            return Err(make_err!(Code::Internal, "Client certificate specified, but no key"));
        };
        read_config.identity(tonic::transport::Identity::from_pem(
            std::fs::read_to_string(client_certificate)?,
            std::fs::read_to_string(client_key)?,
        ))
    } else {
        if config.key_file.is_some() {
            return Err(make_err!(Code::Internal, "Client key specified, but no certificate"));
        }
        read_config
    };

    Ok(Some(config))
}

pub fn endpoint_from(
    endpoint: &str,
    tls_config: Option<tonic::transport::ClientTlsConfig>,
) -> Result<tonic::transport::Endpoint, Error> {
    let endpoint =
        Uri::try_from(endpoint).map_err(|e| make_err!(Code::Internal, "Unable to parse endpoint {endpoint}: {e:?}"))?;

    let endpoint_transport = if let Some(tls_config) = &tls_config {
        let Some(authority) = endpoint.authority() else {
            return Err(make_input_err!("Unable to determine authority of endpont: {endpoint}"));
        };
        let tls_config = tls_config.clone().domain_name(authority.host());
        tonic::transport::Endpoint::from(endpoint)
            .tls_config(tls_config)
            .map_err(|e| make_input_err!("Setting mTLS configuration: {e:?}"))?
    } else {
        tonic::transport::Endpoint::from(endpoint)
    };

    Ok(endpoint_transport)
}
