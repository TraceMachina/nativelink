// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use std::convert::TryFrom;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use buf_channel::{make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf};
use common::DigestInfo;
use error::{make_input_err, Error, ResultExt};
use prometheus_utils::{Collector, CollectorState, MetricsComponent, Registry};
use traits::{StoreTrait, UploadSizeInfo};

pub struct VerifyStore {
    inner_store: Arc<dyn StoreTrait>,
    verify_size: bool,
    verify_hash: bool,

    // Metrics.
    size_verification_failures: AtomicU64,
    hash_verification_failures: AtomicU64,
}

impl VerifyStore {
    pub fn new(config: &config::stores::VerifyStore, inner_store: Arc<dyn StoreTrait>) -> Self {
        VerifyStore {
            inner_store,
            verify_size: config.verify_size,
            verify_hash: config.verify_hash,
            size_verification_failures: AtomicU64::new(0),
            hash_verification_failures: AtomicU64::new(0),
        }
    }

    fn pin_inner(&self) -> Pin<&dyn StoreTrait> {
        Pin::new(self.inner_store.as_ref())
    }

    async fn inner_check_update(
        &self,
        mut tx: DropCloserWriteHalf,
        mut rx: DropCloserReadHalf,
        size_info: UploadSizeInfo,
        mut maybe_hasher: Option<([u8; 32], Sha256)>,
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
                        self.size_verification_failures.fetch_add(1, Ordering::Relaxed);
                        return Err(make_input_err!(
                            "Expected size {} but got size {} on insert",
                            expected_size,
                            sum_size
                        ));
                    }
                }
                if let Some((original_hash, hasher)) = maybe_hasher {
                    let hash_result: [u8; 32] = hasher.finalize().into();
                    if original_hash != hash_result {
                        self.hash_verification_failures.fetch_add(1, Ordering::Relaxed);
                        return Err(make_input_err!(
                            "Hashes do not match, got: {} but digest hash was {}",
                            hex::encode(original_hash),
                            hex::encode(hash_result),
                        ));
                    }
                }
                tx.send_eof().await.err_tip(|| "In verify_store::check_update")?;
                break;
            }

            // This will allows us to hash while sending data to another thread.
            let write_future = tx.send(chunk.clone());

            if let Some((_, hasher)) = maybe_hasher.as_mut() {
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
impl StoreTrait for VerifyStore {
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
        let digest_size =
            usize::try_from(digest.size_bytes).err_tip(|| "Digest size_bytes was not convertible to usize")?;
        if let UploadSizeInfo::ExactSize(expected_size) = size_info {
            if self.verify_size && expected_size != digest_size {
                self.size_verification_failures.fetch_add(1, Ordering::Relaxed);
                return Err(make_input_err!(
                    "Expected size to match. Got {} but digest says {} on update",
                    expected_size,
                    digest.size_bytes
                ));
            }
        }

        let mut hasher = None;
        if self.verify_hash {
            hasher = Some((digest.packed_hash, Sha256::new()));
        }

        let (tx, rx) = make_buf_channel_pair();

        let update_fut = self.pin_inner().update(digest, rx, size_info);
        let check_fut = self.inner_check_update(tx, reader, size_info, hasher);

        let (update_res, check_res) = tokio::join!(update_fut, check_fut);

        update_res.merge(check_res)
    }

    async fn get_part(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        self.pin_inner().get_part(digest, writer, offset, length).await
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
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
            "verify_hash_enabled",
            &self.verify_hash,
            "If the verification store is verifying the hash of the data",
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
