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
use std::sync::Arc;

use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_config::provider_config::ProviderConfig;
use aws_config::{AppName, BehaviorVersion};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Credentials, Region};
use nativelink_config::stores::{ExperimentalAwsSpec, ExperimentalR2Spec};
use nativelink_error::Error;
use nativelink_util::instant_wrapper::InstantWrapper;

use crate::common_s3_utils::TlsClient;
use crate::s3_store::S3Store;

/// R2-shaped config adapter over [`S3Store`]. Builds an S3 SDK client
/// pointed at R2 (custom endpoint, region `auto`, optional explicit
/// credentials) and hands it to `S3Store::new_with_client_and_jitter`.
#[derive(Debug, Clone, Copy)]
pub struct R2Store;

impl R2Store {
    #[allow(clippy::new_ret_no_self)] // Returns a pinned future for async construction.
    pub async fn new<I, NowFn>(
        spec: &ExperimentalR2Spec,
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

        let mut config_loader = aws_config::defaults(BehaviorVersion::latest())
            .app_name(AppName::new("nativelink").expect("valid app name"))
            .timeout_config(
                aws_config::timeout::TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(15))
                    .build(),
            )
            .region(Region::new("auto"))
            .endpoint_url(&endpoint)
            .http_client(http_client.clone());

        config_loader = if let Some(key_id) = &spec.access_key_id
            && let Some(secret) = &spec.secret_access_key
        {
            config_loader.credentials_provider(Credentials::new(
                key_id,
                secret,
                None,
                None,
                "r2-explicit",
            ))
        } else {
            let default_chain = DefaultCredentialsChain::builder()
                .configure(
                    ProviderConfig::without_region()
                        .with_region(Some(Region::new("auto")))
                        .with_http_client(http_client),
                )
                .build()
                .await;
            config_loader.credentials_provider(default_chain)
        };

        let config = config_loader.load().await;
        let s3_client = Client::new(&config);

        S3Store::new_with_client_and_jitter(&aws_spec, s3_client, jitter_fn, now_fn)
    }

    pub fn derive_endpoint(spec: &ExperimentalR2Spec) -> String {
        format!("https://{}.r2.cloudflarestorage.com", spec.account_id)
    }

    pub fn build_aws_spec(spec: &ExperimentalR2Spec) -> ExperimentalAwsSpec {
        ExperimentalAwsSpec {
            region: "auto".to_string(),
            bucket: spec.bucket.clone(),
            common: spec.common.clone(),
        }
    }
}
