// Copyright 2023 The Native Link Authors. All rights reserved.
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

use std::default::Default;
use std::pin::Pin;

use bytes::{Bytes, BytesMut};
use error::{Code, Error, ResultExt};
use futures::future::join;
use futures::{Future, FutureExt};
use native_link_util::buf_channel::{make_buf_channel_pair, DropCloserWriteHalf};
use native_link_util::common::{fs, DigestInfo};
use native_link_util::digest_hasher::DigestHasher;
use native_link_util::store_trait::{Store, UploadSizeInfo};
use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt};

// NOTE(blaise.bruer) From some local testing it looks like action cache items are rarely greater than
// 1.2k. Giving a bit more just in case to reduce allocs.
pub const ESTIMATED_DIGEST_SIZE: usize = 2048;

/// This is more of a safety check. We are going to collect this entire message
/// into memory. If we don't bound the max size of the object we enable users
/// to use up all the memory on this machine.
const MAX_ACTION_MSG_SIZE: usize = 10 << 20; // 10mb.

/// Default read buffer size for reading from an AsyncReader.
const DEFAULT_READ_BUFF_SIZE: usize = 4096;

/// Attempts to fetch the digest contents from a store into the associated proto.
pub async fn get_and_decode_digest<T: Message + Default>(
    store: Pin<&dyn Store>,
    digest: &DigestInfo,
) -> Result<T, Error> {
    let mut store_data_resp = store
        .get_part_unchunked(*digest, 0, Some(MAX_ACTION_MSG_SIZE), Some(ESTIMATED_DIGEST_SIZE))
        .await;
    if let Err(err) = &mut store_data_resp {
        if err.code == Code::NotFound {
            // Trim the error code. Not Found is quite common and we don't want to send a large
            // error (debug) message for something that is common. We resize to just the last
            // message as it will be the most relevant.
            err.messages.resize_with(1, || "".to_string());
        }
    }
    let store_data = store_data_resp?;

    T::decode(store_data).err_tip_with_code(|e| (Code::NotFound, format!("Stored value appears to be corrupt: {}", e)))
}

/// Computes the digest of a message.
pub fn message_to_digest<'a>(
    message: &impl Message,
    mut buf: &mut BytesMut,
    hasher: impl Into<&'a mut DigestHasher>,
) -> Result<DigestInfo, Error> {
    let hasher = hasher.into();
    message
        .encode(&mut buf)
        .err_tip(|| "Could not encode directory proto")?;
    hasher.update(buf);
    Ok(hasher.finalize_digest(i64::try_from(buf.len())?))
}

/// Takes a proto message and will serialize it and upload it to the provided store.
pub async fn serialize_and_upload_message<'a, T: Message>(
    message: &'a T,
    cas_store: Pin<&'a dyn Store>,
    hasher: impl Into<&mut DigestHasher>,
) -> Result<DigestInfo, Error> {
    let mut buffer = BytesMut::with_capacity(message.encoded_len());
    let digest = message_to_digest(message, &mut buffer, hasher).err_tip(|| "In serialize_and_upload_message")?;
    upload_buf_to_store(cas_store, digest, buffer.freeze())
        .await
        .err_tip(|| "In serialize_and_upload_message")?;
    Ok(digest)
}

/// Computes a digest of a given buffer.
pub async fn compute_buf_digest(buf: &[u8], hasher: impl Into<&mut DigestHasher>) -> Result<DigestInfo, Error> {
    let hasher = hasher.into();
    hasher.update(buf);
    Ok(hasher.finalize_digest(i64::try_from(buf.len())?))
}

/// Given a bytestream computes the digest for the data.
pub async fn compute_digest<R: AsyncRead + Unpin + Send>(
    mut reader: R,
    hasher: impl Into<&mut DigestHasher>,
) -> Result<(DigestInfo, R), Error> {
    let mut chunk = BytesMut::with_capacity(DEFAULT_READ_BUFF_SIZE);
    let hasher = hasher.into();
    let mut digest_size = 0;
    loop {
        reader
            .read_buf(&mut chunk)
            .await
            .err_tip(|| "Could not read chunk during compute_digest")?;
        if chunk.is_empty() {
            break; // EOF.
        }
        digest_size += chunk.len();
        hasher.update(&chunk);
        chunk.clear();
    }

    Ok((hasher.finalize_digest(i64::try_from(digest_size)?), reader))
}

fn inner_upload_file_to_store<'a, Fut: Future<Output = Result<(), Error>> + 'a>(
    cas_store: Pin<&'a dyn Store>,
    digest: DigestInfo,
    read_data_fn: impl FnOnce(DropCloserWriteHalf) -> Fut,
) -> impl Future<Output = Result<(), Error>> + 'a {
    let (tx, rx) = make_buf_channel_pair();
    join(
        cas_store
            .update(digest, rx, UploadSizeInfo::ExactSize(digest.size_bytes as usize))
            .map(|r| r.err_tip(|| "Could not upload data to store in upload_file_to_store")),
        read_data_fn(tx),
    )
    // Ensure we get errors reported from both sides
    .map(|(upload_result, read_result)| upload_result.merge(read_result))
}

/// Uploads data to our store for given digest.
pub fn upload_buf_to_store(
    cas_store: Pin<&dyn Store>,
    digest: DigestInfo,
    buf: Bytes,
) -> impl Future<Output = Result<(), Error>> + '_ {
    inner_upload_file_to_store(cas_store, digest, move |mut tx| async move {
        if !buf.is_empty() {
            tx.send(buf)
                .await
                .err_tip(|| "Could not send buffer data to store in upload_buf_to_store")?;
        }
        tx.send_eof()
            .await
            .err_tip(|| "Could not send EOF to store in upload_buf_to_store")
    })
}

/// Same as `upload_buf_to_store`, however it specializes in dealing with a `ResumeableFileSlot`.
/// This will close the reading file to close if writing the data takes a while.
pub fn upload_file_to_store<'a>(
    cas_store: Pin<&'a dyn Store>,
    digest: DigestInfo,
    mut file_reader: fs::ResumeableFileSlot<'a>,
) -> impl Future<Output = Result<(), Error>> + 'a {
    inner_upload_file_to_store(cas_store, digest, move |tx| async move {
        let (_, mut tx) = file_reader
            .read_buf_cb(
                (BytesMut::with_capacity(DEFAULT_READ_BUFF_SIZE), tx),
                move |(chunk, mut tx)| async move {
                    tx.send(chunk.freeze())
                        .await
                        .err_tip(|| "Failed to send in upload_file_to_store")?;
                    Ok((BytesMut::with_capacity(DEFAULT_READ_BUFF_SIZE), tx))
                },
            )
            .await
            .err_tip(|| "Error in upload_file_to_store::read_buf_cb section")?;
        tx.send_eof()
            .await
            .err_tip(|| "Could not send EOF to store in upload_file_to_store")?;
        Ok(())
    })
}
