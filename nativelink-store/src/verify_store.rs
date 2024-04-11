// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nativelink_config::stores::ConfigDigestHashFunction;
use nativelink_error::{make_input_err, Error, ResultExt};
use nativelink_util::buf_channel::{
    make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::health_utils::{default_health_status_indicator, HealthStatusIndicator};
use nativelink_util::metrics_utils::{
    Collector, CollectorState, CounterWithTime, MetricsComponent, Registry,
};
use nativelink_util::store_trait::{Store, UploadSizeInfo};

pub struct VerifyStore {
    inner_store: Arc<dyn Store>,
    verify_size: bool,
    hash_verification_function: Option<ConfigDigestHashFunction>,

    // Metrics.
    size_verification_failures: CounterWithTime,
    hash_verification_failures: CounterWithTime,
}

impl VerifyStore {
    pub fn new(
        config: &nativelink_config::stores::VerifyStore,
        inner_store: Arc<dyn Store>,
    ) -> Self {
        VerifyStore {
            inner_store,
            verify_size: config.verify_size,
            hash_verification_function: config.hash_verification_function,
            size_verification_failures: CounterWithTime::default(),
            hash_verification_failures: CounterWithTime::default(),
        }
    }

    fn pin_inner(&self) -> Pin<&dyn Store> {
        Pin::new(self.inner_store.as_ref())
    }

    async fn inner_check_update<D: DigestHasher>(
        &self,
        mut tx: DropCloserWriteHalf,
        mut rx: DropCloserReadHalf,
        size_info: UploadSizeInfo,
        original_hash: [u8; 32],
        mut maybe_hasher: Option<&mut D>,
    ) -> Result<(), Error> {
        let mut sum_size: u64 = 0;
        loop {
            let chunk = rx
                .recv()
                .await
                .err_tip(|| "Failed to reach chunk in check_update in verify store")?;
            sum_size += chunk.len() as u64;

            if chunk.is_empty() {
                // Is EOF.
                if let UploadSizeInfo::ExactSize(expected_size) = size_info {
                    if sum_size != expected_size as u64 {
                        self.size_verification_failures.inc();
                        return Err(make_input_err!(
                            "Expected size {} but got size {} on insert",
                            expected_size,
                            sum_size
                        ));
                    }
                }
                if let Some(hasher) = maybe_hasher {
                    let hash_result: [u8; 32] = hasher.finalize_digest().packed_hash;
                    if original_hash != hash_result {
                        self.hash_verification_failures.inc();
                        return Err(make_input_err!(
                            "Hashes do not match, got: {} but digest hash was {}",
                            hex::encode(original_hash),
                            hex::encode(hash_result),
                        ));
                    }
                }
                tx.send_eof().err_tip(|| "In verify_store::check_update")?;
                break;
            }

            // This will allows us to hash while sending data to another thread.
            let write_future = tx.send(chunk.clone());

            if let Some(hasher) = maybe_hasher.as_mut() {
                hasher.update(chunk.as_ref());
            }

            write_future
                .await
                .err_tip(|| "Failed to write chunk to inner store in verify store")?;
        }
        Ok(())
    }
}

#[async_trait]
impl Store for VerifyStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        self.pin_inner().has_with_results(digests, results).await
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let digest_size = usize::try_from(digest.size_bytes)
            .err_tip(|| "Digest size_bytes was not convertible to usize")?;
        if let UploadSizeInfo::ExactSize(expected_size) = size_info {
            if self.verify_size && expected_size != digest_size {
                self.size_verification_failures.inc();
                return Err(make_input_err!(
                    "Expected size to match. Got {} but digest says {} on update",
                    expected_size,
                    digest.size_bytes
                ));
            }
        }

        let mut hasher = self
            .hash_verification_function
            .map(|v| DigestHasherFunc::from(v).hasher());

        let (tx, rx) = make_buf_channel_pair();

        let update_fut = self.pin_inner().update(digest, rx, size_info);
        let check_fut =
            self.inner_check_update(tx, reader, size_info, digest.packed_hash, hasher.as_mut());

        let (update_res, check_res) = tokio::join!(update_fut, check_fut);

        update_res.merge(check_res)
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        self.pin_inner()
            .get_part_ref(digest, writer, offset, length)
            .await
    }

    fn inner_store(&self, _digest: Option<DigestInfo>) -> &'_ dyn Store {
        self
    }

    fn inner_store_arc(self: Arc<Self>, _digest: Option<DigestInfo>) -> Arc<dyn Store> {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn std::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_metrics(self: Arc<Self>, registry: &mut Registry) {
        registry.register_collector(Box::new(Collector::new(&self)));
    }
}

impl MetricsComponent for VerifyStore {
    fn gather_metrics(&self, c: &mut CollectorState) {
        c.publish(
            "verify_size_enabled",
            &self.verify_size,
            "If the verification store is verifying the size of the data",
        );
        c.publish(
            "hash_verification_function",
            &format!("{:?}", self.hash_verification_function),
            "Hash verification function to verify the contents of the data",
        );
        c.publish(
            "size_verification_failures_total",
            &self.size_verification_failures,
            "Number of failures the verification store had due to size mismatches",
        );
        c.publish(
            "hash_verification_failures_total",
            &self.hash_verification_failures,
            "Number of failures the verification store had due to hash mismatches",
        );
    }
}

default_health_status_indicator!(VerifyStore);
