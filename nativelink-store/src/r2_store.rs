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

use std::sync::Arc;

use nativelink_config::stores::{ExperimentalAwsSpec, ExperimentalR2Spec};
use nativelink_error::Error;
use nativelink_util::instant_wrapper::InstantWrapper;

use crate::s3_store::S3Store;

/// R2-shaped config adapter over [`S3Store`] (R2 is wire-compatible with S3).
#[derive(Debug, Clone, Copy)]
pub struct R2Store;

impl R2Store {
    pub async fn new<I, NowFn>(
        spec: &ExperimentalR2Spec,
        now_fn: NowFn,
    ) -> Result<Arc<S3Store<NowFn>>, Error>
    where
        I: InstantWrapper,
        NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
    {
        S3Store::new(&Self::to_aws_spec(spec), now_fn).await
    }

    pub fn to_aws_spec(spec: &ExperimentalR2Spec) -> ExperimentalAwsSpec {
        ExperimentalAwsSpec {
            region: "auto".to_string(),
            bucket: spec.bucket.clone(),
            endpoint: Some(format!(
                "https://{}.r2.cloudflarestorage.com",
                spec.account_id
            )),
            access_key_id: spec.access_key_id.clone(),
            secret_access_key: spec.secret_access_key.clone(),
            common: spec.common.clone(),
        }
    }
}
