// Copyright 2026 Trace Machina, Inc. All rights reserved.
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

use aws_config::{AppName, BehaviorVersion};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::retry::RetryConfig;
use aws_sdk_s3::config::{
    Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_smithy_runtime_api::client::http::HttpClient as SmithyHttpClient;
use nativelink_config::stores::{CommonObjectSpec, ExperimentalAwsSpec, ExperimentalOssSpec};
use nativelink_error::{Code, Error, make_err};
use nativelink_util::instant_wrapper::InstantWrapper;

use crate::common_s3_utils::TlsClient;
use crate::s3_store::S3Store;

/// OSS S3-compatible endpoint adapter. Transfer logic stays in `S3Store`.
/// See <https://www.alibabacloud.com/help/en/oss/developer-reference/use-aws-sdks-to-access-oss>.
#[derive(Debug, Clone, Copy)]
pub struct OssStore;

impl OssStore {
    #[allow(clippy::new_ret_no_self)]
    pub fn new<I, NowFn>(
        spec: &ExperimentalOssSpec,
        now_fn: NowFn,
    ) -> Result<Arc<S3Store<NowFn>>, Error>
    where
        I: InstantWrapper,
        NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
    {
        let aws_spec = Self::build_aws_spec(spec)?;
        let http_client = TlsClient::new(&aws_spec.common)?;
        Self::new_with_http_client(spec, http_client, now_fn)
    }

    /// Allows wire tests to supply an HTTP client without contacting OSS.
    #[allow(clippy::new_ret_no_self)]
    pub fn new_with_http_client<I, NowFn, C>(
        spec: &ExperimentalOssSpec,
        http_client: C,
        now_fn: NowFn,
    ) -> Result<Arc<S3Store<NowFn>>, Error>
    where
        I: InstantWrapper,
        NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
        C: SmithyHttpClient + Clone + 'static,
    {
        let aws_spec = Self::build_aws_spec(spec)?;
        let region = Region::new(spec.region.clone());
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .app_name(AppName::new("nativelink").expect("valid app name"))
            .region(region)
            .endpoint_url(Self::endpoint(spec)?)
            // OSS documents virtual-hosted addressing for its S3 interface.
            .force_path_style(false)
            // Do not add optional trailing checksums / aws-chunked encoding.
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .timeout_config(
                aws_config::timeout::TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(15))
                    .operation_attempt_timeout(Duration::from_mins(2))
                    .build(),
            )
            .credentials_provider(Credentials::new(
                &spec.access_key_id,
                &spec.secret_access_key,
                spec.session_token.clone(),
                None,
                "oss-config",
            ))
            .http_client(http_client)
            .build();
        S3Store::new_with_client_and_jitter(
            &aws_spec,
            Client::from_conf(config),
            spec.retry.make_jitter_fn(),
            now_fn,
        )
    }

    pub fn endpoint(spec: &ExperimentalOssSpec) -> Result<String, Error> {
        let endpoint = spec
            .endpoint
            .clone()
            .unwrap_or_else(|| format!("https://s3.oss-{}.aliyuncs.com", spec.region));
        let url = url::Url::parse(&endpoint)
            .map_err(|_| make_err!(Code::InvalidArgument, "Invalid OSS endpoint"))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(make_err!(
                Code::InvalidArgument,
                "OSS endpoint must be an HTTPS service URL without credentials, path, query or fragment"
            ));
        }
        Ok(endpoint)
    }

    pub fn build_aws_spec(spec: &ExperimentalOssSpec) -> Result<ExperimentalAwsSpec, Error> {
        if spec.region.is_empty()
            || !spec
                .region
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(make_err!(Code::InvalidArgument, "Invalid OSS region"));
        }
        let bucket = spec.bucket.as_bytes();
        if !(3..=63).contains(&bucket.len())
            || !bucket[0].is_ascii_alphanumeric()
            || !bucket[bucket.len() - 1].is_ascii_alphanumeric()
            || !bucket
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        {
            return Err(make_err!(Code::InvalidArgument, "Invalid OSS bucket name"));
        }
        if spec.access_key_id.trim().is_empty() || spec.secret_access_key.trim().is_empty() {
            return Err(make_err!(
                Code::InvalidArgument,
                "OSS requires explicit access_key_id and secret_access_key"
            ));
        }
        let concurrency = spec.multipart_max_concurrent_uploads.unwrap_or(2);
        if concurrency == 0 || concurrency > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(make_err!(
                Code::InvalidArgument,
                "Invalid OSS multipart concurrency"
            ));
        }
        Self::endpoint(spec)?;
        Ok(ExperimentalAwsSpec {
            region: spec.region.clone(),
            bucket: spec.bucket.clone(),
            common: CommonObjectSpec {
                key_prefix: spec.key_prefix.clone(),
                retry: spec.retry.clone(),
                multipart_max_concurrent_uploads: Some(concurrency),
                ..Default::default()
            },
            ..Default::default()
        })
    }
}
