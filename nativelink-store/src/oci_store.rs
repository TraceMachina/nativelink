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

use core::time::Duration;
use std::borrow::Cow;
use std::sync::Arc;

use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_config::provider_config::ProviderConfig;
use aws_config::{AppName, BehaviorVersion};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Credentials, Region};
use nativelink_config::stores::{ExperimentalAwsSpec, ExperimentalOciSpec};
use nativelink_error::Error;
use nativelink_util::instant_wrapper::InstantWrapper;

use crate::common_s3_utils::TlsClient;
use crate::s3_store::S3Store;

/// OCI-shaped config adapter over [`S3Store`]. Builds an S3 SDK client pointed
/// at Oracle Cloud Infrastructure Object Storage via its Amazon S3
/// Compatibility API and hands it to `S3Store::new_with_client_and_jitter`.
///
/// The compatibility API requires path-style addressing: the namespace lives in
/// the host and the bucket is the first path segment
/// (`https://{namespace}.compat.objectstorage.{region}.oci.customer-oci.com/{bucket}/{object}`),
/// so `force_path_style(true)` is set on the client. Requests are signed with
/// AWS `SigV4` using the OCI region as the signing region; credentials come from
/// a Customer Secret Key when supplied, otherwise the default AWS credential
/// chain.
#[derive(Debug, Clone, Copy)]
pub struct OciStore;

impl OciStore {
    #[allow(clippy::new_ret_no_self)] // Because usually everyone returns themselves
    pub async fn new<I, NowFn>(
        spec: &ExperimentalOciSpec,
        now_fn: NowFn,
    ) -> Result<Arc<S3Store<NowFn>>, Error>
    where
        I: InstantWrapper,
        NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
    {
        let aws_spec = Self::build_aws_spec(spec);
        let jitter_fn = spec.common.retry.make_jitter_fn();

        let http_client = TlsClient::new(&spec.common.clone());
        let endpoint = Self::derive_endpoint(spec);
        let region = Region::new(Cow::Owned(spec.region.clone()));

        let mut config_builder = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .app_name(AppName::new("nativelink").expect("valid app name"))
            .timeout_config(
                aws_config::timeout::TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(15))
                    .build(),
            )
            .region(region.clone())
            .endpoint_url(&endpoint)
            // OCI's compatibility endpoint embeds the bucket in the path, not as
            // a subdomain, so virtual-hosted addressing must be disabled.
            .force_path_style(true)
            .http_client(http_client.clone());

        config_builder = if let Some(key_id) = &spec.access_key_id
            && let Some(secret) = &spec.secret_access_key
        {
            config_builder.credentials_provider(Credentials::new(
                key_id,
                secret,
                None,
                None,
                "oci-customer-secret-key",
            ))
        } else {
            let default_chain = DefaultCredentialsChain::builder()
                .configure(
                    ProviderConfig::without_region()
                        .with_region(Some(region))
                        .with_http_client(http_client),
                )
                .build()
                .await;
            config_builder.credentials_provider(default_chain)
        };

        let s3_client = Client::from_conf(config_builder.build());

        S3Store::new_with_client_and_jitter(&aws_spec, s3_client, jitter_fn, now_fn)
    }

    /// Derives the OCI Object Storage path-style S3-compatibility endpoint from
    /// the namespace and region.
    pub fn derive_endpoint(spec: &ExperimentalOciSpec) -> String {
        format!(
            "https://{}.compat.objectstorage.{}.oci.customer-oci.com",
            spec.namespace, spec.region
        )
    }

    pub fn build_aws_spec(spec: &ExperimentalOciSpec) -> ExperimentalAwsSpec {
        ExperimentalAwsSpec {
            region: spec.region.clone(),
            bucket: spec.bucket.clone(),
            common: spec.common.clone(),
        }
    }
}
