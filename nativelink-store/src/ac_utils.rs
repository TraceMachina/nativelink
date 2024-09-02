// Copyright 2024 The NativeLink Authors. All rights reserved.
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

// @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
// TODO(aaronmondal): IMPORTANT TODO: IMPORTING THIS SOMETMIES BREAKS
//                    THREADSAFETY. FIGURE OUT WHY AND MOVE IT TO UTILS.
// @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@

use std::pin::Pin;

use bytes::BytesMut;
use futures::TryFutureExt;
use nativelink_error::{Code, Error, ResultExt};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasher;
use nativelink_util::store_trait::{StoreKey, StoreLike};
use prost::Message;

// NOTE(blaise.bruer) From some local testing it looks like action cache items are rarely greater than
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
) -> Result<(T, usize), Error> {
    let key = key.into();
    // Note: For unknown reasons we appear to be hitting:
    // https://github.com/rust-lang/rust/issues/92096
    // or a smiliar issue if we try to use the non-store driver function, so we
    // are using the store driver function here.
    let mut store_data_resp = store
        .as_store_driver_pin()
        .get_part_unchunked(key.borrow(), 0, Some(MAX_ACTION_MSG_SIZE))
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
    let store_data_len = store_data.len();

    T::decode(store_data)
        .err_tip_with_code(|e| {
            (
                Code::NotFound,
                format!("Stored value appears to be corrupt: {e} - {key:?}"),
            )
        })
        .map(|v| (v, store_data_len))
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
    // Note: For unknown reasons we appear to be hitting:
    // https://github.com/rust-lang/rust/issues/92096
    // or a smiliar issue if we try to use the non-store driver function, so we
    // are using the store driver function here.
    cas_store
        .as_store_driver_pin()
        .update_oneshot(digest.into(), buffer.freeze())
        .await
        .err_tip(|| "In serialize_and_upload_message")?;
    Ok(digest)
}

/// Computes a digest of a given buffer.
pub fn compute_buf_digest(buf: &[u8], hasher: &mut impl DigestHasher) -> DigestInfo {
    hasher.update(buf);
    hasher.finalize_digest()
}
