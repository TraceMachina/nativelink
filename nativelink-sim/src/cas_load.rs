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

//! The CAS load generator (RFC-0061 §3.1): the three budgets of TESTING §2
//! against one CAS endpoint. Each phase checks what it gets back, not only
//! how long it took: `FindMissingBlobs` must name exactly the blobs that
//! were never uploaded, and every read must hash to its digest.

use core::fmt;
use core::future::Future;
use core::time::Duration;
use std::collections::HashSet;

use bytes::Bytes;
use futures::{StreamExt, stream};
use nativelink_error::{Code, Error, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::batch_update_blobs_request::Request as BlobRequest;
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_client::ContentAddressableStorageClient;
use nativelink_proto::build::bazel::remote::execution::v2::{
    BatchUpdateBlobsRequest, FindMissingBlobsRequest, digest_function,
};
use nativelink_proto::google::bytestream::byte_stream_client::ByteStreamClient;
use nativelink_proto::google::bytestream::{ReadRequest, WriteRequest};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use tokio::time::Instant;
use tonic::transport::Channel;

use crate::property::Rng;

/// What one run drives.
#[derive(Clone, Debug)]
pub struct CasLoad {
    pub instance: String,
    pub seed: u64,
    /// Digests in the one `FindMissingBlobs` call, a tenth of them uploaded
    /// first so the answer has both kinds.
    pub find_missing_digests: usize,
    /// Large blobs uploaded at once through `ByteStream`, then read back.
    pub large_uploads: usize,
    pub large_mib: usize,
    /// Small blobs uploaded, then read at once.
    pub reads: usize,
    pub read_kib: usize,
}

/// Latencies of one phase.
#[derive(Clone, Copy, Debug, Default)]
pub struct Phase {
    pub name: &'static str,
    pub count: usize,
    pub errors: usize,
    pub total: Duration,
    pub p50: Duration,
    pub p95: Duration,
    pub max: Duration,
}

impl Phase {
    fn new(
        name: &'static str,
        total: Duration,
        mut latencies: Vec<Duration>,
        errors: usize,
    ) -> Self {
        latencies.sort();
        let at = |percent: usize| {
            latencies
                .get(latencies.len().saturating_sub(1) * percent / 100)
                .copied()
                .unwrap_or_default()
        };
        Self {
            name,
            count: latencies.len(),
            errors,
            total,
            p50: at(50),
            p95: at(95),
            max: at(100),
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: n={} errors={} total={}ms p50={}ms p95={}ms max={}ms",
            self.name,
            self.count,
            self.errors,
            self.total.as_millis(),
            self.p50.as_millis(),
            self.p95.as_millis(),
            self.max.as_millis(),
        )
    }
}

/// One MiB of the bytes of blob `blob`, block `block`: a function of the
/// seed, so a reader can check what it got without holding the blob.
fn block(seed: u64, blob: u64, block: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed ^ blob.rotate_left(32) ^ block.wrapping_mul(0x9e37_79b9));
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        out.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    out.truncate(len);
    out
}

const BLOCK: usize = 1024 * 1024;

/// The digest of a generated blob of `size` bytes, hashed block by block.
fn generated_digest(seed: u64, blob: u64, size: usize) -> DigestInfo {
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    let mut offset = 0;
    let mut index = 0;
    while offset < size {
        let len = BLOCK.min(size - offset);
        hasher.update(&block(seed, blob, index, len));
        offset += len;
        index += 1;
    }
    hasher.finalize_digest()
}

fn digest_of(bytes: &[u8]) -> DigestInfo {
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(bytes);
    hasher.finalize_digest()
}

/// `instance/` as a resource name prefix: none for the empty instance, as
/// Bazel sends it.
fn prefix(instance: &str) -> String {
    if instance.is_empty() {
        String::new()
    } else {
        format!("{instance}/")
    }
}

fn blob_name(instance: &str, digest: &DigestInfo) -> String {
    format!(
        "{}blobs/{}/{}",
        prefix(instance),
        digest.packed_hash(),
        digest.size_bytes()
    )
}

async fn upload_small(
    cas: &mut ContentAddressableStorageClient<Channel>,
    instance: &str,
    blobs: &[(DigestInfo, Vec<u8>)],
) -> Result<(), Error> {
    // Batched by bytes, as Bazel does: a request over the server's 4 MiB
    // message limit is refused whole.
    const BATCH_BYTES: usize = 3 * 1024 * 1024;
    let mut batches: Vec<&[(DigestInfo, Vec<u8>)]> = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (i, (_, data)) in blobs.iter().enumerate() {
        if i > start && bytes + data.len() > BATCH_BYTES {
            batches.push(&blobs[start..i]);
            start = i;
            bytes = 0;
        }
        bytes += data.len();
    }
    if start < blobs.len() {
        batches.push(&blobs[start..]);
    }
    for chunk in batches {
        let response = cas
            .batch_update_blobs(BatchUpdateBlobsRequest {
                instance_name: instance.to_string(),
                requests: chunk
                    .iter()
                    .map(|(digest, data)| BlobRequest {
                        digest: Some((*digest).into()),
                        data: Bytes::from(data.clone()),
                        compressor: 0,
                    })
                    .collect(),
                digest_function: digest_function::Value::Sha256.into(),
            })
            .await
            .map_err(|e| make_err!(Code::Internal, "BatchUpdateBlobs failed: {e}"))?
            .into_inner();
        if let Some(bad) = response
            .responses
            .iter()
            .find(|r| r.status.as_ref().is_some_and(|s| s.code != 0))
        {
            return Err(make_err!(
                Code::Internal,
                "BatchUpdateBlobs refused a blob: {bad:?}"
            ));
        }
    }
    Ok(())
}

/// One `FindMissingBlobs` over `n` digests, a tenth uploaded first; the
/// answer must be exactly the other nine tenths.
async fn find_missing(
    cas: &mut ContentAddressableStorageClient<Channel>,
    load: &CasLoad,
) -> Result<Phase, Error> {
    let n = load.find_missing_digests;
    let mut present = Vec::new();
    let mut digests = Vec::with_capacity(n);
    for i in 0..n {
        let data = format!("fleet-sim-fmb-{}-{i}", load.seed).into_bytes();
        let digest = digest_of(&data);
        if i % 10 == 0 {
            present.push((digest, data));
        }
        digests.push(digest);
    }
    upload_small(cas, &load.instance, &present).await?;
    let expected: HashSet<DigestInfo> = digests
        .iter()
        .copied()
        .filter(|d| !present.iter().any(|(p, _)| p == d))
        .collect();

    let start = Instant::now();
    let response = cas
        .find_missing_blobs(FindMissingBlobsRequest {
            instance_name: load.instance.clone(),
            blob_digests: digests.iter().map(|d| (*d).into()).collect(),
            digest_function: digest_function::Value::Sha256.into(),
        })
        .await
        .map_err(|e| make_err!(Code::Internal, "FindMissingBlobs failed: {e}"))?
        .into_inner();
    let took = start.elapsed();
    let missing: HashSet<DigestInfo> = response
        .missing_blob_digests
        .iter()
        .filter_map(|d| DigestInfo::try_from(d).ok())
        .collect();
    let wrong = missing.symmetric_difference(&expected).count();
    Ok(Phase::new("find-missing", took, vec![took], wrong))
}

/// Streams one generated blob through `ByteStream` `Write`.
async fn write_large(
    mut bytestream: ByteStreamClient<Channel>,
    instance: String,
    seed: u64,
    blob: u64,
    size: usize,
) -> Result<DigestInfo, Error> {
    let digest = generated_digest(seed, blob, size);
    let resource = format!(
        "{}uploads/{:032x}/blobs/{}/{}",
        prefix(&instance),
        u128::from(seed) << 64 | u128::from(blob),
        digest.packed_hash(),
        digest.size_bytes()
    );
    let blocks = size.div_ceil(BLOCK);
    let requests = stream::iter(0..blocks).map(move |index| {
        let offset = index * BLOCK;
        let len = BLOCK.min(size - offset);
        WriteRequest {
            resource_name: if index == 0 {
                resource.clone()
            } else {
                String::new()
            },
            write_offset: i64::try_from(offset).unwrap_or(i64::MAX),
            finish_write: index + 1 == blocks,
            data: Bytes::from(block(seed, blob, index as u64, len)),
        }
    });
    let response = bytestream
        .write(requests)
        .await
        .map_err(|e| make_err!(Code::Internal, "ByteStream write failed: {e}"))?
        .into_inner();
    if usize::try_from(response.committed_size).ok() != Some(size) {
        return Err(make_err!(
            Code::Internal,
            "committed {} of {size} bytes",
            response.committed_size
        ));
    }
    Ok(digest)
}

/// Reads `digest` through `ByteStream` and checks it hashes to itself.
async fn read_and_verify(
    mut bytestream: ByteStreamClient<Channel>,
    instance: String,
    digest: DigestInfo,
) -> Result<(), Error> {
    let mut stream = bytestream
        .read(ReadRequest {
            resource_name: blob_name(&instance, &digest),
            read_offset: 0,
            read_limit: 0,
        })
        .await
        .map_err(|e| make_err!(Code::Internal, "ByteStream read failed: {e}"))?
        .into_inner();
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| make_err!(Code::Internal, "Read stream failed: {e}"))?;
        hasher.update(&chunk.data);
    }
    let got = hasher.finalize_digest();
    if got != digest {
        return Err(make_err!(Code::DataLoss, "read {got}, expected {digest}"));
    }
    Ok(())
}

/// Runs `jobs` at once and times each; errors are counted, not fatal.
async fn timed<F>(jobs: Vec<F>) -> (Duration, Vec<Duration>, Vec<Error>)
where
    F: Future<Output = Result<(), Error>> + Send + 'static,
{
    let start = Instant::now();
    let handles: Vec<_> = jobs
        .into_iter()
        .map(|job| {
            nativelink_util::background_spawn!("fleet_sim_cas_job", async move {
                let begun = Instant::now();
                job.await.map(|()| begun.elapsed())
            })
        })
        .collect();
    let mut latencies = Vec::new();
    let mut errors = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok(took)) => latencies.push(took),
            Ok(Err(err)) => errors.push(err),
            Err(err) => errors.push(make_err!(Code::Internal, "Job panicked: {err}")),
        }
    }
    (start.elapsed(), latencies, errors)
}

/// The three phases, in order. `channels` spreads the streams over several
/// connections.
pub async fn run(load: &CasLoad, channels: &[Channel]) -> Result<(Vec<Phase>, Vec<Error>), Error> {
    let channel = |i: usize| channels[i % channels.len()].clone();
    let mut cas = ContentAddressableStorageClient::new(channel(0))
        .max_decoding_message_size(64 * 1024 * 1024)
        .max_encoding_message_size(64 * 1024 * 1024);
    let mut phases = Vec::new();
    let mut failures = Vec::new();

    if load.find_missing_digests > 0 {
        phases.push(find_missing(&mut cas, load).await?);
    }

    if load.large_uploads > 0 {
        let size = load.large_mib * BLOCK;
        let jobs: Vec<_> = (0..load.large_uploads)
            .map(|i| {
                let client = ByteStreamClient::new(channel(i));
                let instance = load.instance.clone();
                let seed = load.seed;
                async move {
                    let digest =
                        write_large(client.clone(), instance.clone(), seed, i as u64, size).await?;
                    read_and_verify(client, instance, digest).await
                }
            })
            .collect();
        let (total, latencies, errors) = timed(jobs).await;
        phases.push(Phase::new(
            "large-write-read",
            total,
            latencies,
            errors.len(),
        ));
        failures.extend(errors);
    }

    if load.reads > 0 {
        let blobs: Vec<_> = (0..load.reads)
            .map(|i| {
                let data = block(load.seed, 1_000_000 + i as u64, 0, load.read_kib * 1024);
                (digest_of(&data), data)
            })
            .collect();
        upload_small(&mut cas, &load.instance, &blobs).await?;
        let jobs: Vec<_> = blobs
            .iter()
            .enumerate()
            .map(|(i, (digest, _))| {
                let client = ByteStreamClient::new(channel(i));
                let instance = load.instance.clone();
                let digest = *digest;
                async move { read_and_verify(client, instance, digest).await }
            })
            .collect();
        let (total, latencies, errors) = timed(jobs).await;
        phases.push(Phase::new(
            "concurrent-reads",
            total,
            latencies,
            errors.len(),
        ));
        failures.extend(errors);
    }
    Ok((phases, failures))
}

/// The bytes of generated blob `blob`, whole, for tests of the generator.
pub fn generated_bytes(seed: u64, blob: u64, size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size);
    let mut index = 0;
    while out.len() < size {
        let len = BLOCK.min(size - out.len());
        out.extend(block(seed, blob, index, len));
        index += 1;
    }
    out
}

/// The digest the generator streams for blob `blob`, for tests.
pub fn generated_digest_of(seed: u64, blob: u64, size: usize) -> DigestInfo {
    generated_digest(seed, blob, size)
}
