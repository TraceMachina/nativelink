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

use core::time::Duration;
use std::borrow::Cow;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_config::provider_config::ProviderConfig;
use aws_config::{AppName, BehaviorVersion};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{ProvideCredentials, Region, RequestChecksumCalculation};
use aws_smithy_runtime_api::client::http::HttpClient as SmithyHttpClient;
use hyper_rustls::HttpsConnectorBuilder;
use nativelink_config::stores::{ExperimentalAwsSpec, ExperimentalOntapS3Spec};
use nativelink_error::{Code, Error, ResultExt};
use nativelink_util::instant_wrapper::InstantWrapper;
use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;

use crate::common_s3_utils::{TlsClient, install_default_rustls_crypto_provider};
use crate::s3_store::S3Store;

pub fn load_custom_certs(cert_path: &str) -> Result<Arc<ClientConfig>, Error> {
    let mut root_store = RootCertStore::empty();

    // Create a BufReader from the cert file
    let mut cert_reader = BufReader::new(
        File::open(cert_path)
            .err_tip(|| format!("Failed to open CA certificate file {cert_path}"))?,
    );

    // Parse certificates
    let certs = CertificateDer::pem_reader_iter(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;

    // Add each certificate to the root store
    for cert in certs {
        root_store.add(cert).map_err(|e| {
            Error::from_std_err(Code::Internal, &e)
                .append("Failed to add certificate to root store")
        })?;
    }

    // Build the client config with the root store
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    Ok(Arc::new(config))
}

/// ONTAP-shaped config adapter over [`S3Store`]. Builds an S3 SDK client
/// pointed at ONTAP (path-style, vserver as region, optional custom CA)
/// and hands it to `S3Store::new_with_client_and_jitter`.
#[derive(Debug, Clone, Copy)]
pub struct OntapS3Store;

impl OntapS3Store {
    #[allow(clippy::new_ret_no_self)] // Because usually everyone returns themselves
    pub async fn new<I, NowFn>(
        spec: &ExperimentalOntapS3Spec,
        now_fn: NowFn,
    ) -> Result<Arc<S3Store<NowFn>>, Error>
    where
        I: InstantWrapper,
        NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
    {
        let s3_client = Self::make_client(spec).await?;
        Self::new_with_client(spec, s3_client, now_fn)
    }

    #[allow(clippy::new_ret_no_self)]
    pub fn new_with_client<I, NowFn>(
        spec: &ExperimentalOntapS3Spec,
        s3_client: Client,
        now_fn: NowFn,
    ) -> Result<Arc<S3Store<NowFn>>, Error>
    where
        I: InstantWrapper,
        NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
    {
        S3Store::new_with_client_and_jitter(
            &Self::build_aws_spec(spec),
            s3_client,
            spec.common.retry.make_jitter_fn(),
            now_fn,
        )
    }

    /// Also used by the existence cache to list the bucket.
    pub async fn make_client(spec: &ExperimentalOntapS3Spec) -> Result<Client, Error> {
        let credentials_provider = DefaultCredentialsChain::builder()
            .configure(
                ProviderConfig::without_region()
                    .with_region(Some(Self::region(spec)))
                    .with_http_client(TlsClient::new_for_credentials(&spec.common)?),
            )
            .build()
            .await;

        Ok(Self::make_client_with(
            spec,
            Self::make_http_client(spec)?,
            credentials_provider,
        ))
    }

    /// Tests inject a mock HTTP client and fixed credentials here.
    pub fn make_client_with(
        spec: &ExperimentalOntapS3Spec,
        http_client: impl SmithyHttpClient + 'static,
        credentials_provider: impl ProvideCredentials + 'static,
    ) -> Client {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .app_name(AppName::new("nativelink").expect("valid app name"))
            .timeout_config(
                aws_config::timeout::TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(30))
                    .operation_timeout(Duration::from_mins(2))
                    .build(),
            )
            .region(Self::region(spec))
            .endpoint_url(&spec.endpoint)
            .force_path_style(true)
            // Avoid `aws-chunked` uploads, not yet confirmed to work on ONTAP.
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .credentials_provider(credentials_provider)
            .http_client(http_client)
            .build();

        Client::from_conf(config)
    }

    pub fn build_aws_spec(spec: &ExperimentalOntapS3Spec) -> ExperimentalAwsSpec {
        ExperimentalAwsSpec {
            region: spec.vserver_name.clone(),
            bucket: spec.bucket.clone(),
            common: spec.common.clone(),
            ..Default::default()
        }
    }

    fn region(spec: &ExperimentalOntapS3Spec) -> Region {
        Region::new(Cow::Owned(spec.vserver_name.clone()))
    }

    /// With `root_certificates` set, only that CA bundle is trusted.
    fn make_http_client(spec: &ExperimentalOntapS3Spec) -> Result<TlsClient, Error> {
        let Some(cert_path) = &spec.root_certificates else {
            return TlsClient::new(&spec.common);
        };

        install_default_rustls_crypto_provider();
        let ca_config = load_custom_certs(cert_path)?;

        let connector_with_roots =
            HttpsConnectorBuilder::new().with_tls_config((*ca_config).clone());
        let connector_with_schemes = if spec.common.insecure_allow_http {
            connector_with_roots.https_or_http()
        } else {
            connector_with_roots.https_only()
        };
        let connector = if spec.common.disable_http2 {
            connector_with_schemes.enable_http1().build()
        } else {
            connector_with_schemes.enable_http1().enable_http2().build()
        };

        Ok(TlsClient::with_https_connector(&spec.common, connector))
    }
}
