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

use std::sync::{Arc, OnceLock};

use blake3::Hasher as Blake3Hasher;
use bytes::BytesMut;
use futures::Future;
use nativelink_config::stores::ConfigDigestHashFunction;
use nativelink_error::{make_err, make_input_err, Code, Error, ResultExt};
use nativelink_proto::build::bazel::remote::execution::v2::digest_function::Value as ProtoDigestFunction;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::common::DigestInfo;
use crate::origin_context::{ActiveOriginContext, OriginContext};
use crate::{fs, make_symbol, spawn_blocking};

// The symbol can be use to retrieve the active hasher function.
// from an `OriginContext`.
make_symbol!(ACTIVE_HASHER_FUNC, DigestHasherFunc);

static DEFAULT_DIGEST_HASHER_FUNC: OnceLock<DigestHasherFunc> = OnceLock::new();

/// Utility function to make a context with a specific hasher function set.
pub fn make_ctx_for_hash_func<H>(hasher: H) -> Result<Arc<OriginContext>, Error>
where
    H: TryInto<DigestHasherFunc>,
    H::Error: Into<Error>,
{
    let digest_hasher_func = hasher
        .try_into()
        .err_tip(|| "Could not convert into DigestHasherFunc")?;

    let mut new_ctx = ActiveOriginContext::fork().err_tip(|| "In BytestreamServer::inner_write")?;
    new_ctx.set_value(&ACTIVE_HASHER_FUNC, Arc::new(digest_hasher_func));
    Ok(Arc::new(new_ctx))
}

/// Get the default hasher.
pub fn default_digest_hasher_func() -> DigestHasherFunc {
    *DEFAULT_DIGEST_HASHER_FUNC.get_or_init(|| DigestHasherFunc::Sha256)
}

/// Sets the default hasher to use if no hasher was requested by the client.
pub fn set_default_digest_hasher_func(hasher: DigestHasherFunc) -> Result<(), Error> {
    DEFAULT_DIGEST_HASHER_FUNC
        .set(hasher)
        .map_err(|_| make_err!(Code::Internal, "default_digest_hasher_func already set"))
}

/// Supported digest hash functions.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum DigestHasherFunc {
    Sha256,
    Blake3,
}

impl DigestHasherFunc {
    pub fn hasher(&self) -> DigestHasherImpl {
        self.into()
    }

    #[must_use]
    pub const fn proto_digest_func(&self) -> ProtoDigestFunction {
        match self {
            Self::Sha256 => ProtoDigestFunction::Sha256,
            Self::Blake3 => ProtoDigestFunction::Blake3,
        }
    }
}

impl From<ConfigDigestHashFunction> for DigestHasherFunc {
    fn from(value: ConfigDigestHashFunction) -> Self {
        match value {
            ConfigDigestHashFunction::sha256 => Self::Sha256,
            ConfigDigestHashFunction::blake3 => Self::Blake3,
        }
    }
}

impl TryFrom<ProtoDigestFunction> for DigestHasherFunc {
    type Error = Error;

    fn try_from(value: ProtoDigestFunction) -> Result<Self, Self::Error> {
        match value {
            ProtoDigestFunction::Sha256 => Ok(Self::Sha256),
            ProtoDigestFunction::Blake3 => Ok(Self::Blake3),
            v => Err(make_input_err!(
                "Unknown or unsupported digest function for proto conversion {v:?}"
            )),
        }
    }
}

impl TryFrom<&str> for DigestHasherFunc {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_uppercase().as_str() {
            "SHA256" => Ok(Self::Sha256),
            "BLAKE3" => Ok(Self::Blake3),
            v => Err(make_input_err!(
                "Unknown or unsupported digest function for string conversion: {v:?}"
            )),
        }
    }
}

impl std::fmt::Display for DigestHasherFunc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DigestHasherFunc::Sha256 => write!(f, "SHA256"),
            DigestHasherFunc::Blake3 => write!(f, "BLAKE3"),
        }
    }
}

impl TryFrom<i32> for DigestHasherFunc {
    type Error = Error;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        // Zero means not-set.
        if value == 0 {
            return Ok(default_digest_hasher_func());
        }
        match ProtoDigestFunction::try_from(value) {
            Ok(ProtoDigestFunction::Sha256) => Ok(Self::Sha256),
            Ok(ProtoDigestFunction::Blake3) => Ok(Self::Blake3),
            value => Err(make_input_err!(
                "Unknown or unsupported digest function for int conversion: {:?}",
                value.map(|v| v.as_str_name())
            )),
        }
    }
}

impl From<&DigestHasherFunc> for DigestHasherImpl {
    fn from(value: &DigestHasherFunc) -> Self {
        let hash_func_impl = match value {
            DigestHasherFunc::Sha256 => DigestHasherFuncImpl::Sha256(Sha256::new()),
            DigestHasherFunc::Blake3 => DigestHasherFuncImpl::Blake3(Box::new(Blake3Hasher::new())),
        };
        Self {
            hashed_size: 0,
            hash_func_impl,
        }
    }
}

/// Wrapper to compute a hash of arbitrary data.
pub trait DigestHasher {
    /// Update the hasher with some additional data.
    fn update(&mut self, input: &[u8]);

    /// Finalize the hash function and collect the results into a digest.
    fn finalize_digest(&mut self) -> DigestInfo;

    /// Specialized version of the hashing function that is optimized for
    /// handling files. These optimizations take into account things like,
    /// the file size and the hasher algorithm to decide how to best process
    /// the file and feed it into the hasher.
    fn digest_for_file(
        self,
        file: fs::ResumeableFileSlot,
        size_hint: Option<u64>,
    ) -> impl Future<Output = Result<(DigestInfo, fs::ResumeableFileSlot), Error>>;

    /// Utility function to compute a hash from a generic reader.
    fn compute_from_reader<R: AsyncRead + Unpin + Send>(
        &mut self,
        mut reader: R,
    ) -> impl Future<Output = Result<DigestInfo, Error>> {
        async move {
            let mut chunk = BytesMut::with_capacity(fs::DEFAULT_READ_BUFF_SIZE);
            loop {
                reader
                    .read_buf(&mut chunk)
                    .await
                    .err_tip(|| "Could not read chunk during compute_from_reader")?;
                if chunk.is_empty() {
                    break; // EOF.
                }
                DigestHasher::update(self, &chunk);
                chunk.clear();
            }
            Ok(DigestHasher::finalize_digest(self))
        }
    }
}

pub enum DigestHasherFuncImpl {
    Sha256(Sha256),
    Blake3(Box<Blake3Hasher>), // Box because Blake3Hasher is 1.3kb in size.
}

/// The individual implementation of the hash function.
pub struct DigestHasherImpl {
    hashed_size: i64,
    hash_func_impl: DigestHasherFuncImpl,
}

impl DigestHasherImpl {
    #[inline]
    async fn hash_file(
        &mut self,
        mut file: fs::ResumeableFileSlot,
    ) -> Result<(DigestInfo, fs::ResumeableFileSlot), Error> {
        let reader = file.as_reader().await.err_tip(|| "In digest_for_file")?;
        let digest = self
            .compute_from_reader(reader)
            .await
            .err_tip(|| "In digest_for_file")?;
        Ok((digest, file))
    }
}

impl DigestHasher for DigestHasherImpl {
    #[inline]
    fn update(&mut self, input: &[u8]) {
        self.hashed_size += input.len() as i64;
        match &mut self.hash_func_impl {
            DigestHasherFuncImpl::Sha256(h) => sha2::digest::Update::update(h, input),
            DigestHasherFuncImpl::Blake3(h) => {
                Blake3Hasher::update(h, input);
            }
        }
    }

    #[inline]
    fn finalize_digest(&mut self) -> DigestInfo {
        let hash = match &mut self.hash_func_impl {
            DigestHasherFuncImpl::Sha256(h) => h.finalize_reset().into(),
            DigestHasherFuncImpl::Blake3(h) => h.finalize().into(),
        };
        DigestInfo::new(hash, self.hashed_size)
    }

    async fn digest_for_file(
        mut self,
        mut file: fs::ResumeableFileSlot,
        size_hint: Option<u64>,
    ) -> Result<(DigestInfo, fs::ResumeableFileSlot), Error> {
        let file_position = file
            .stream_position()
            .await
            .err_tip(|| "Couldn't get stream position in digest_for_file")?;
        if file_position != 0 {
            return self.hash_file(file).await;
        }
        // If we are a small file, it's faster to just do it the "slow" way.
        // Great read: https://github.com/david-slatinek/c-read-vs.-mmap
        if let Some(size_hint) = size_hint {
            if size_hint <= fs::DEFAULT_READ_BUFF_SIZE as u64 {
                return self.hash_file(file).await;
            }
        }
        match self.hash_func_impl {
            DigestHasherFuncImpl::Sha256(_) => self.hash_file(file).await,
            DigestHasherFuncImpl::Blake3(mut hasher) => {
                spawn_blocking!("digest_for_file", move || {
                    hasher.update_mmap(file.get_path()).map_err(|e| {
                        make_err!(Code::Internal, "Error in blake3's update_mmap: {e:?}")
                    })?;
                    Result::<_, Error>::Ok((
                        DigestInfo::new(hasher.finalize().into(), hasher.count() as i64),
                        file,
                    ))
                })
                .await
                .err_tip(|| "Could not spawn blocking task in digest_for_file")?
            }
        }
    }
}
