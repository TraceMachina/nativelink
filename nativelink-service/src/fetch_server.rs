// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use core::convert::Into;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use nativelink_config::cas_server::{FetchConfig, FetchDownloaderConfig, WithInstanceName};
use nativelink_error::{Error, ResultExt, make_err, make_input_err};
use nativelink_proto::build::bazel::remote::asset::v1::fetch_server::{
    Fetch, FetchServer as Server,
};
use nativelink_proto::build::bazel::remote::asset::v1::{
    FetchBlobRequest, FetchBlobResponse, FetchDirectoryRequest, FetchDirectoryResponse,
};
use nativelink_proto::google::rpc::Status as GoogleStatus;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{
    DigestHasher, DigestHasherFunc, default_digest_hasher_func, digest_hasher_func_from_context,
    make_ctx_for_hash_func,
};
use nativelink_util::store_trait::{Store, StoreLike, UploadSizeInfo};
use opentelemetry::context::FutureExt;
use prost::Message;
use reqwest::{Client, NoProxy, Proxy};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use tonic::{Code, Request, Response, Status};
use tracing::{Instrument, Level, error_span, info, instrument};

use crate::remote_asset_proto::{RemoteAssetArtifact, RemoteAssetQuery};

#[derive(Debug, Clone)]
pub struct FetchStoreInfo {
    store: Store,
    downloader: Option<Arc<Downloader>>,
}

#[derive(Debug)]
struct ExpectedChecksum {
    function: DigestHasherFunc,
    hash: [u8; 32],
}

#[derive(Debug)]
struct Downloader {
    client: Client,
    proxy_client: Option<Client>,
    require_checksum: bool,
    allow_unknown_qualifiers: bool,
    cas: Store,
    max_size: Option<u64>,
    timeout: Duration,
    slots: Semaphore,
}

impl Downloader {
    fn new(config: &FetchDownloaderConfig, stores: &StoreManager) -> Result<Self, Error> {
        if config.http_proxy.is_some() && config.http_proxy_secret_file.is_some() {
            return Err(make_input_err!("Set only one fetch proxy URL source"));
        }
        if config.max_concurrent_downloads > Semaphore::MAX_PERMITS {
            return Err(make_input_err!(
                "Fetch max_concurrent_downloads is too large"
            ));
        }
        let proxy_url = match &config.http_proxy_secret_file {
            Some(path) => Some(
                std::fs::read_to_string(path)
                    .map_err(|_| make_input_err!("Could not read fetch proxy secret file"))?,
            ),
            None => config.http_proxy.clone(),
        };
        // Match the server's TLS provider, including memory-only store configurations.
        drop(rustls::crypto::ring::default_provider().install_default());
        let builder = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::limited(10));
        let proxy_client = if let Some(url) = proxy_url {
            let url = url.trim();
            let parsed =
                reqwest::Url::parse(url).map_err(|_| make_input_err!("Invalid fetch proxy URL"))?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err(make_input_err!("Fetch proxy must be an HTTP(S) URL"));
            }
            let bypass = NoProxy::from_string(&config.no_proxy.join(","));
            let proxy = Proxy::all(url)
                .map_err(|_| make_input_err!("Invalid fetch proxy URL"))?
                .no_proxy(bypass);
            Some(
                Client::builder()
                    .redirect(reqwest::redirect::Policy::limited(10))
                    .no_proxy()
                    .proxy(proxy)
                    .build()
                    .map_err(|_| make_input_err!("Could not build fetch proxy client"))?,
            )
        } else {
            None
        };
        let cas = stores
            .get_store(&config.cas_store)
            .ok_or_else(|| make_input_err!("Fetch downloader CAS store does not exist"))?;
        Ok(Self {
            client: builder
                .build()
                .map_err(|_| make_input_err!("Could not build fetch HTTP client"))?,
            cas,
            proxy_client,
            require_checksum: config.require_checksum,
            allow_unknown_qualifiers: config.allow_unknown_qualifiers,
            max_size: (config.max_size_bytes != 0).then_some(config.max_size_bytes),
            timeout: Duration::from_secs(if config.timeout_s == 0 {
                300
            } else {
                config.timeout_s
            }),
            slots: Semaphore::new(if config.max_concurrent_downloads == 0 {
                4
            } else {
                config.max_concurrent_downloads
            }),
        })
    }

    async fn download(
        &self,
        uri: &str,
        checksum: Option<&[ExpectedChecksum]>,
    ) -> Result<DigestInfo, Error> {
        let default = Box::pin(self.download_with_client(&self.client, uri, checksum));
        let Some(proxy) = &self.proxy_client else {
            return default.await;
        };
        // A failed path does not cancel an alternative that can still succeed.
        let attempts = vec![
            default,
            Box::pin(self.download_with_client(proxy, uri, checksum)),
        ];
        futures::future::select_ok(attempts)
            .await
            .map(|(digest, _)| digest)
    }

    async fn download_with_client(
        &self,
        client: &Client,
        uri: &str,
        checksum: Option<&[ExpectedChecksum]>,
    ) -> Result<DigestInfo, Error> {
        let url = reqwest::Url::parse(uri).map_err(|_| make_input_err!("Invalid fetch URI"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(make_input_err!("Only HTTP(S) fetch URIs are supported"));
        }
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|_| make_err!(Code::Unavailable, "Origin request failed"))?
            .error_for_status()
            .map_err(|_| make_err!(Code::Unavailable, "Origin returned an HTTP error"))?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(make_err!(
                Code::Unavailable,
                "Origin did not return a complete object"
            ));
        }
        if self
            .max_size
            .is_some_and(|limit| response.content_length().is_some_and(|size| size > limit))
        {
            return Err(make_err!(
                Code::ResourceExhausted,
                "Fetch object exceeds max_size_bytes"
            ));
        }
        // Spool before publishing: no unverified object enters CAS, even without a checksum.
        // The anonymous file is removed on close, including timeout and cancellation.
        let file = nativelink_util::spawn_blocking!("fetch_spool_create", tempfile::tempfile)
            .await
            .map_err(|_| make_err!(Code::Internal, "Could not create fetch spool"))??;
        let mut file = tokio::fs::File::from_std(file);
        let digest_function = digest_hasher_func_from_context();
        let mut hasher = digest_function.hasher();
        let mut checksum_hashers = Vec::new();
        for expected in checksum.into_iter().flatten() {
            if expected.function != digest_function
                && !checksum_hashers
                    .iter()
                    .any(|(function, _)| *function == expected.function)
            {
                checksum_hashers.push((expected.function, expected.function.hasher()));
            }
        }
        let mut size = 0_u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| make_err!(Code::Unavailable, "Origin body transfer failed"))?
        {
            size = size
                .checked_add(chunk.len() as u64)
                .filter(|n| self.max_size.is_none_or(|limit| *n <= limit))
                .ok_or_else(|| {
                    make_err!(
                        Code::ResourceExhausted,
                        "Fetch object exceeds max_size_bytes"
                    )
                })?;
            hasher.update(&chunk);
            for (_, hasher) in &mut checksum_hashers {
                hasher.update(&chunk);
            }
            file.write_all(&chunk).await?;
        }
        let digest = hasher.finalize_digest();
        if let Some(alternatives) = checksum {
            let checksum_digests: Vec<_> = checksum_hashers
                .into_iter()
                .map(|(function, mut hasher)| (function, hasher.finalize_digest()))
                .collect();
            let matches = alternatives.iter().any(|expected| {
                let actual = if expected.function == digest_function {
                    Some(&digest)
                } else {
                    checksum_digests
                        .iter()
                        .find(|(function, _)| *function == expected.function)
                        .map(|(_, digest)| digest)
                };
                actual.is_some_and(|actual| {
                    actual.packed_hash().as_slice() == expected.hash.as_slice()
                })
            });
            if !matches {
                return Err(make_err!(Code::DataLoss, "Fetch checksum mismatch"));
            }
        }
        file.rewind().await?;
        let (mut tx, rx) = make_buf_channel_pair();
        let replay = async {
            let mut buffer = vec![0; 64 * 1024];
            loop {
                let count = file.read(&mut buffer).await?;
                if count == 0 {
                    break;
                }
                tx.send(Bytes::copy_from_slice(&buffer[..count])).await?;
            }
            tx.send_eof()
        };
        futures::try_join!(
            replay,
            self.cas.update(digest, rx, UploadSizeInfo::ExactSize(size))
        )?;
        Ok(digest)
    }
}

#[derive(Debug, Clone)]
pub struct FetchServer {
    stores: HashMap<String, FetchStoreInfo>,
}

impl FetchServer {
    pub fn new(
        configs: &[WithInstanceName<FetchConfig>],
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut stores = HashMap::with_capacity(configs.len());
        for config in configs {
            let store = store_manager
                .get_store(&config.fetch_store)
                .ok_or_else(|| {
                    make_input_err!("'fetch_store': '{}' does not exist", config.fetch_store)
                })?;
            let downloader = config
                .downloader
                .as_ref()
                .map(|spec| Downloader::new(spec, store_manager).map(Arc::new))
                .transpose()?;
            stores.insert(
                config.instance_name.clone(),
                FetchStoreInfo { store, downloader },
            );
        }
        Ok(Self { stores })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    async fn inner_fetch_blob(
        &self,
        request: FetchBlobRequest,
    ) -> Result<Response<FetchBlobResponse>, Error> {
        let instance_name = &request.instance_name;
        let store_info = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?;

        if request.uris.is_empty() {
            return Err(Error::new(
                Code::InvalidArgument,
                "No uris in fetch request".to_owned(),
            ));
        }
        let download_options = if let Some(downloader) = &store_info.downloader {
            let checksum = Self::checksum(&request, downloader)?;
            let timeout = match request.timeout {
                Some(value) => Duration::try_from(value)
                    .map_err(|_| make_input_err!("Invalid fetch timeout"))?
                    .min(downloader.timeout),
                None => downloader.timeout,
            };
            Some((downloader, checksum, timeout))
        } else {
            None
        };
        for uri in &request.uris {
            let asset_request = RemoteAssetQuery::new(uri.clone(), request.qualifiers.clone());
            let asset_digest = asset_request.digest();
            let asset_response_possible = store_info
                .store
                .get_part_unchunked(asset_digest, 0, None)
                .await;

            info!(
                digest = format!("{}", asset_digest),
                "Looked up fetch asset"
            );

            if let Ok(asset_response_raw) = asset_response_possible {
                let asset_response = RemoteAssetArtifact::decode(asset_response_raw)
                    .err_tip(|| "Failed to decode stored RemoteAssetArtifact")?;
                if let Some(downloader) = &store_info.downloader {
                    // Legacy index keys concatenate fields without boundaries. A matching
                    // key alone does not prove that the record belongs to this request.
                    if asset_response.uri != asset_request.uri
                        || asset_response.qualifiers != asset_request.qualifiers
                    {
                        continue;
                    }
                    let now: prost_types::Timestamp = SystemTime::now().into();
                    let unexpired = asset_response.expire_at.as_ref().is_none_or(|expiry| {
                        (expiry.seconds, expiry.nanos) > (now.seconds, now.nanos)
                    });
                    let stored_function =
                        DigestHasherFunc::try_from(asset_response.digest_function)?;
                    let same_function = stored_function == digest_hasher_func_from_context();
                    let fresh_enough =
                        request
                            .oldest_content_accepted
                            .as_ref()
                            .is_none_or(|oldest| {
                                asset_response.fetched_at.as_ref().is_some_and(|fetched| {
                                    (fetched.seconds, fetched.nanos)
                                        >= (oldest.seconds, oldest.nanos)
                                })
                            });
                    if !unexpired || !same_function || !fresh_enough {
                        continue;
                    }
                    let Some(blob) = asset_response.blob_digest.as_ref() else {
                        continue;
                    };
                    if downloader
                        .cas
                        .has(DigestInfo::try_from(blob)?)
                        .await?
                        .is_none()
                    {
                        continue;
                    }
                }
                return Ok(Response::new(FetchBlobResponse {
                    status: Some(GoogleStatus {
                        code: Code::Ok.into(),
                        message: "Fetch object found".to_owned(),
                        details: vec![],
                    }),
                    uri: asset_response.uri,
                    qualifiers: asset_response.qualifiers,
                    expires_at: asset_response.expire_at,
                    blob_digest: asset_response.blob_digest,
                    digest_function: asset_response.digest_function,
                }));
            }
        }
        let Some((downloader, checksum, timeout)) = download_options else {
            let mut response =
                Self::miss_response(&request, make_err!(Code::NotFound, "No item found"));
            response.digest_function = default_digest_hasher_func().proto_digest_func().into();
            return Ok(Response::new(response));
        };
        let retrieve = async {
            let _slot = downloader
                .slots
                .acquire()
                .await
                .map_err(|_| make_err!(Code::Unavailable, "Fetch downloader closed"))?;
            let mut last_error = make_err!(Code::NotFound, "No downloadable URI found");
            for uri in &request.uris {
                let fetched_at = SystemTime::now().into();
                match downloader.download(uri, checksum.as_deref()).await {
                    Ok(digest) => {
                        let mut artifact = RemoteAssetArtifact::new(
                            uri.clone(),
                            request.qualifiers.clone(),
                            digest.into(),
                            None,
                            digest_hasher_func_from_context().proto_digest_func().into(),
                        );
                        artifact.fetched_at = Some(fetched_at);
                        store_info
                            .store
                            .update_oneshot(artifact.digest(), artifact.as_bytes())
                            .await?;
                        return Ok(Response::new(FetchBlobResponse {
                            status: Some(GoogleStatus {
                                code: 0,
                                message: "Fetch object created".into(),
                                details: vec![],
                            }),
                            uri: artifact.uri,
                            qualifiers: artifact.qualifiers,
                            expires_at: artifact.expire_at,
                            blob_digest: artifact.blob_digest,
                            digest_function: artifact.digest_function,
                        }));
                    }
                    Err(error) => last_error = error,
                }
            }
            Ok(Response::new(Self::miss_response(&request, last_error)))
        };
        tokio::time::timeout(timeout, retrieve)
            .await
            .unwrap_or_else(|_| {
                Ok(Response::new(Self::miss_response(
                    &request,
                    make_err!(Code::DeadlineExceeded, "Fetch origin timeout exceeded"),
                )))
            })
    }

    fn checksum(
        request: &FetchBlobRequest,
        downloader: &Downloader,
    ) -> Result<Option<Vec<ExpectedChecksum>>, Error> {
        let mut checksum = None;
        for qualifier in &request.qualifiers {
            if qualifier.name != "checksum.sri" {
                if qualifier.name != "bazel.canonical_id" && !downloader.allow_unknown_qualifiers {
                    return Err(make_input_err!("Unsupported fetch qualifier"));
                }
                continue;
            }
            if checksum.is_some() {
                return Err(make_input_err!("Duplicate checksum qualifier"));
            }
            let mut alternatives = Vec::new();
            for token in qualifier.value.split_whitespace() {
                let (algorithm, encoded) = token
                    .split_once('-')
                    .ok_or_else(|| make_input_err!("Invalid checksum.sri format"))?;
                if algorithm.is_empty() || encoded.is_empty() {
                    return Err(make_input_err!("Invalid checksum.sri format"));
                }
                let hash = BASE64
                    .decode(encoded)
                    .map_err(|_| make_input_err!("Invalid checksum.sri encoding"))?;
                // Unsupported algorithms may coexist with supported alternatives.
                let Ok(function) = DigestHasherFunc::try_from(algorithm) else {
                    continue;
                };
                let hash = hash
                    .try_into()
                    .map_err(|_| make_input_err!("Invalid checksum.sri length"))?;
                alternatives.push(ExpectedChecksum { function, hash });
            }
            if alternatives.is_empty() {
                return Err(make_input_err!("No supported checksum.sri alternative"));
            }
            checksum = Some(alternatives);
        }
        if downloader.require_checksum && checksum.is_none() {
            return Err(make_input_err!("Fetch downloader requires checksum.sri"));
        }
        Ok(checksum)
    }

    fn miss_response(request: &FetchBlobRequest, error: Error) -> FetchBlobResponse {
        FetchBlobResponse {
            status: Some(error.into()),
            uri: request.uris.first().cloned().unwrap_or_default(),
            qualifiers: vec![],
            expires_at: None,
            blob_digest: None,
            digest_function: digest_hasher_func_from_context().proto_digest_func().into(),
        }
    }
}

#[tonic::async_trait]
impl Fetch for FetchServer {
    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err(level = Level::WARN),
        skip_all,
        fields(
            uri_count = grpc_request.get_ref().uris.len(),
            qualifier_count = grpc_request.get_ref().qualifiers.len(),
            digest_function = grpc_request.get_ref().digest_function,
        )
    )]
    async fn fetch_blob(
        &self,
        grpc_request: Request<FetchBlobRequest>,
    ) -> Result<Response<FetchBlobResponse>, Status> {
        let request = grpc_request.into_inner();
        let digest_function = request.digest_function;
        self.inner_fetch_blob(request)
            .instrument(error_span!("fetch_server_fetch_blob"))
            .with_context(
                make_ctx_for_hash_func(digest_function).err_tip(|| "In FetchServer::fetch_blob")?,
            )
            .await
            .err_tip(|| "Failed on fetch_blob() command")
            .map_err(Into::into)
    }

    #[allow(clippy::blocks_in_conditions)]
    #[instrument(
        err(level = Level::WARN),
        skip_all,
        fields(
            uri_count = _grpc_request.get_ref().uris.len(),
            qualifier_count = _grpc_request.get_ref().qualifiers.len(),
            digest_function = _grpc_request.get_ref().digest_function,
        )
    )]
    async fn fetch_directory(
        &self,
        _grpc_request: Request<FetchDirectoryRequest>,
    ) -> Result<Response<FetchDirectoryResponse>, Status> {
        Err(Status::unimplemented("FetchDirectory not implemented"))
    }
}
