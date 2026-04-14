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

// @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
// TODO(palfrey): IMPORTANT TODO: IMPORTING THIS SOMETIMES BREAKS
//                    THREADSAFETY. FIGURE OUT WHY AND MOVE IT TO UTILS.
// @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@

use core::pin::Pin;

use bytes::BytesMut;
use futures::TryFutureExt;
use nativelink_error::{Code, Error, ResultExt};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasher;
use nativelink_util::log_utils::throughput_mbps;
use nativelink_util::store_trait::{StoreKey, StoreLike};
use prost::Message;
use tracing::info;

// NOTE(aaronmondal) From some local testing it looks like action cache items are rarely greater than
// 1.2k. Giving a bit more just in case to reduce allocs.
pub const ESTIMATED_DIGEST_SIZE: usize = 2048;

/// This is more of a safety check. We are going to collect this entire message
/// into memory. If we don't bound the max size of the object we enable users
/// to use up all the memory on this machine.
const MAX_ACTION_MSG_SIZE: usize = 10 << 20; // 10mb.

/// Attempts to fetch the digest contents from a store into the associated proto.
pub async fn get_and_decode_digest<T: Message + Default + 'static>(
    store: &impl StoreLike,
    key: StoreKey<'_>,
) -> Result<T, Error> {
    get_size_and_decode_digest(store, key)
        .map_ok(|(v, _)| v)
        .await
}

/// Attempts to fetch the digest contents from a store into the associated proto.
pub async fn get_size_and_decode_digest<T: Message + Default + 'static>(
    store: &impl StoreLike,
    key: impl Into<StoreKey<'_>>,
) -> Result<(T, u64), Error> {
    let key = key.into();
    // Note: For unknown reasons we appear to be hitting:
    // https://github.com/rust-lang/rust/issues/92096
    // or a smiliar issue if we try to use the non-store driver function, so we
    // are using the store driver function here.
    let mut store_data_resp = store
        .as_store_driver_pin()
        .get_part_unchunked(key.borrow(), 0, Some(MAX_ACTION_MSG_SIZE as u64))
        .await;
    if let Err(err) = &mut store_data_resp {
        if err.code == Code::NotFound {
            // Trim the error code. Not Found is quite common and we don't want to send a large
            // error (debug) message for something that is common. We resize to just the last
            // message as it will be the most relevant.
            err.messages.resize_with(1, String::new);
        }
    }
    let store_data = store_data_resp?;
    let store_data_len =
        u64::try_from(store_data.len()).err_tip(|| "Could not convert store_data.len() to u64")?;

    T::decode(store_data)
        .err_tip_with_code(|e| {
            (
                Code::NotFound,
                format!("Stored value appears to be corrupt: {e} - {key:?}"),
            )
        })
        .map(|v| (v, store_data_len))
}

/// Batch-fetches and decodes multiple digests in a single store operation.
/// Returns results in the same order as the input digests. Uses
/// [`StoreDriver::batch_get_part_unchunked`] which pipelines the underlying
/// I/O when the store supports it (e.g. Redis).
pub async fn batch_get_and_decode_digest<T: Message + Default + 'static>(
    store: &impl StoreLike,
    digests: &[DigestInfo],
) -> Vec<(DigestInfo, Result<T, Error>)> {
    if digests.is_empty() {
        return Vec::new();
    }

    let keys: Vec<_> = digests.iter().map(|d| StoreKey::Digest(*d)).collect();
    let raw_results = store
        .as_store_driver_pin()
        .batch_get_part_unchunked(keys, Some(MAX_ACTION_MSG_SIZE as u64))
        .await;

    digests
        .iter()
        .zip(raw_results)
        .map(|(digest, result)| {
            let decoded = match result {
                Ok(data) => T::decode(data).err_tip_with_code(|e| {
                    (
                        Code::NotFound,
                        format!("Stored value appears to be corrupt: {e} - {digest:?}"),
                    )
                }),
                Err(mut err) => {
                    if err.code == Code::NotFound {
                        err.messages.resize_with(1, String::new);
                    }
                    Err(err)
                }
            };
            (*digest, decoded)
        })
        .collect()
}

/// Computes the digest of a message.
pub fn message_to_digest(
    message: &impl Message,
    mut buf: &mut BytesMut,
    hasher: &mut impl DigestHasher,
) -> Result<DigestInfo, Error> {
    message
        .encode(&mut buf)
        .err_tip(|| "Could not encode directory proto")?;
    hasher.update(buf);
    Ok(hasher.finalize_digest())
}

/// Takes a proto message and will serialize it and upload it to the provided store.
pub async fn serialize_and_upload_message<'a, T: Message>(
    message: &'a T,
    cas_store: Pin<&'a impl StoreLike>,
    hasher: &mut impl DigestHasher,
) -> Result<DigestInfo, Error> {
    let mut buffer = BytesMut::with_capacity(message.encoded_len());
    let digest = message_to_digest(message, &mut buffer, hasher)
        .err_tip(|| "In serialize_and_upload_message")?;
    let size_bytes = buffer.len() as u64;
    // Note: For unknown reasons we appear to be hitting:
    // https://github.com/rust-lang/rust/issues/92096
    // or a smiliar issue if we try to use the non-store driver function, so we
    // are using the store driver function here.
    let start = std::time::Instant::now();
    cas_store
        .as_store_driver_pin()
        .update_oneshot(digest.into(), buffer.freeze())
        .await
        .err_tip(|| "In serialize_and_upload_message")?;
    let elapsed = start.elapsed();
    info!(
        ?digest,
        size_bytes,
        elapsed_ms = elapsed.as_millis() as u64,
        throughput_mbps = format!("{:.1}", throughput_mbps(size_bytes, elapsed)),
        "serialize_and_upload_message: CAS write completed",
    );
    Ok(digest)
}

/// Computes a digest of a given buffer.
pub fn compute_buf_digest(buf: &[u8], hasher: &mut impl DigestHasher) -> DigestInfo {
    hasher.update(buf);
    hasher.finalize_digest()
}
