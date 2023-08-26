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

use std::default::Default;
use std::io::Cursor;
use std::pin::Pin;

use bytes::BytesMut;
use futures::{future::try_join, Future, FutureExt, TryFutureExt};
use prost::Message;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::timeout;

use buf_channel::{make_buf_channel_pair, DropCloserWriteHalf};
use common::{fs, DigestInfo};
use error::{Code, Error, ResultExt};
use fs::idle_file_descriptor_timeout;
use store::{Store, UploadSizeInfo};

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

/// Takes a proto message and will serialize it and upload it to the provided store.
pub async fn serialize_and_upload_message<'a, T: Message>(
    message: &'a T,
    cas_store: Pin<&'a dyn Store>,
) -> Result<DigestInfo, Error> {
    let mut buffer = BytesMut::new();
    let digest = {
        message
            .encode(&mut buffer)
            .err_tip(|| "Could not encode directory proto")?;
        let mut hasher = Sha256::new();
        hasher.update(&buffer);
        DigestInfo::new(hasher.finalize().into(), buffer.len() as i64)
    };
    upload_to_store(cas_store, digest, &mut Cursor::new(buffer)).await?;
    Ok(digest)
}

/// Given a bytestream computes the digest for the data.
/// Note: This will happen in a new spawn since computing digests can be thread intensive.
pub async fn compute_digest<R: AsyncRead + Unpin + Send>(mut reader: R) -> Result<(DigestInfo, R), Error> {
    let mut chunk = BytesMut::with_capacity(DEFAULT_READ_BUFF_SIZE);
    let mut hasher = Sha256::new();
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

    Ok((DigestInfo::new(hasher.finalize().into(), digest_size as i64), reader))
}

fn inner_upload_file_to_store<'a, Fut: Future<Output = Result<(), Error>> + 'a>(
    cas_store: Pin<&'a dyn Store>,
    digest: DigestInfo,
    read_data_fn: impl FnOnce(DropCloserWriteHalf) -> Fut,
) -> impl Future<Output = Result<(), Error>> + 'a {
    let (tx, rx) = make_buf_channel_pair();
    let upload_file_to_store_fut = cas_store
        .update(digest, rx, UploadSizeInfo::ExactSize(digest.size_bytes as usize))
        .map(|r| r.err_tip(|| "Could not upload data to store in upload_file_to_store"));
    try_join(read_data_fn(tx), upload_file_to_store_fut).map_ok(|(_, _)| ())
}

/// Uploads data to our store for given digest.
/// Sadly we cannot upload our data while computing our hash, this means that we often
/// will need to read the file two times, one to hash the file and the other to upload
/// it. In the future we could possibly upload to store while computing the hash and
/// then "finish" the upload by giving the digest, but not all stores will support this
/// for now we will just always read twice.
pub fn upload_to_store<'a, R: AsyncRead + Unpin>(
    cas_store: Pin<&'a dyn Store>,
    digest: DigestInfo,
    reader: &'a mut R,
) -> impl Future<Output = Result<(), Error>> + 'a {
    inner_upload_file_to_store(cas_store, digest, move |mut tx| async move {
        loop {
            let mut chunk = BytesMut::with_capacity(DEFAULT_READ_BUFF_SIZE);
            reader
                .read_buf(&mut chunk)
                .await
                .err_tip(|| "Could not read chunk during upload_to_store")?;
            if chunk.is_empty() {
                break; // EOF.
            }
            tx.send(chunk.freeze())
                .await
                .err_tip(|| "Could not send buffer data to store in upload_to_store")?;
        }
        tx.send_eof()
            .await
            .err_tip(|| "Could not send EOF to store in upload_to_store")?;
        Ok(())
    })
}

/// Same as `upload_to_store`, however it specializes in dealing with a `ResumeableFileSlot`.
/// This will close the reading file to close if writing the data takes a while.
pub fn upload_file_to_store<'a>(
    cas_store: Pin<&'a dyn Store>,
    digest: DigestInfo,
    mut file_reader: fs::ResumeableFileSlot<'a>,
) -> impl Future<Output = Result<(), Error>> + 'a {
    inner_upload_file_to_store(cas_store, digest, move |mut tx| async move {
        loop {
            let mut chunk = BytesMut::with_capacity(DEFAULT_READ_BUFF_SIZE);
            file_reader
                .as_reader()
                .await
                .err_tip(|| "Could not get reader from file slot in upload_file_to_store")?
                .read_buf(&mut chunk)
                .await
                .err_tip(|| "Could not read chunk during upload_file_to_store")?;
            if chunk.is_empty() {
                break; // EOF.
            }
            let send_fut = tx.send(chunk.freeze());
            tokio::pin!(send_fut);
            loop {
                match timeout(idle_file_descriptor_timeout(), &mut send_fut).await {
                    Ok(Ok(())) => break,
                    Ok(Err(err)) => {
                        return Err(err).err_tip(|| "Could not send buffer data to store in upload_file_to_store")
                    }
                    Err(_) => {
                        file_reader
                            .close_file()
                            .await
                            .err_tip(|| "Could not close file due to timeout in upload_file_to_store")?;
                        continue;
                    }
                }
            }
        }
        tx.send_eof()
            .await
            .err_tip(|| "Could not send EOF to store in upload_file_to_store")?;
        Ok(())
    })
}
