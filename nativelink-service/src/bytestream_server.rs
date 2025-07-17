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

use core::convert::Into;
use core::fmt::{Debug, Formatter};
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use futures::future::pending;
use futures::stream::unfold;
use futures::{Future, Stream, TryFutureExt, try_join};
use nativelink_config::cas_server::{ByteStreamConfig, WithInstanceName};
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_proto::google::bytestream::byte_stream_server::{
    ByteStream, ByteStreamServer as Server,
};
use nativelink_proto::google::bytestream::{
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use nativelink_store::grpc_store::GrpcStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{
    DigestHasherFunc, default_digest_hasher_func, make_ctx_for_hash_func,
};
use nativelink_util::proto_stream_utils::WriteRequestStreamWrapper;
use nativelink_util::resource_info::ResourceInfo;
use nativelink_util::spawn;
use nativelink_util::store_trait::{Store, StoreLike, UploadSizeInfo};
use nativelink_util::task::JoinHandleDropGuard;
use opentelemetry::context::FutureExt;
use parking_lot::Mutex;
use tokio::time::sleep;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Instrument, Level, debug, error, error_span, info, instrument, trace};

/// If this value changes update the documentation in the config definition.
const DEFAULT_PERSIST_STREAM_ON_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// If this value changes update the documentation in the config definition.
const DEFAULT_MAX_BYTES_PER_STREAM: usize = 64 * 1024;

/// If this value changes update the documentation in the config definition.
const DEFAULT_MAX_DECODING_MESSAGE_SIZE: usize = 4 * 1024 * 1024;

type ReadStream = Pin<Box<dyn Stream<Item = Result<ReadResponse, Status>> + Send + 'static>>;
type StoreUpdateFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'static>>;

struct StreamState {
    uuid: String,
    tx: DropCloserWriteHalf,
    store_update_fut: StoreUpdateFuture,
}

impl Debug for StreamState {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StreamState")
            .field("uuid", &self.uuid)
            .finish()
    }
}

/// If a stream is in this state, it will automatically be put back into an `IdleStream` and
/// placed back into the `active_uploads` map as an `IdleStream` after it is dropped.
/// To prevent it from being put back into an `IdleStream` you must call `.graceful_finish()`.
struct ActiveStreamGuard<'a> {
    stream_state: Option<StreamState>,
    bytes_received: Arc<AtomicU64>,
    bytestream_server: &'a ByteStreamServer,
    timeout: Duration,
}

impl ActiveStreamGuard<'_> {
    /// Consumes the guard. The stream will be considered "finished", will
    /// remove it from the `active_uploads`.
    fn graceful_finish(mut self) {
        let stream_state = self.stream_state.take().unwrap();
        self.bytestream_server
            .active_uploads
            .lock()
            .remove(&stream_state.uuid);
    }
}

impl Drop for ActiveStreamGuard<'_> {
    fn drop(&mut self) {
        let Some(stream_state) = self.stream_state.take() else {
            return; // If None it means we don't want it put back into an IdleStream.
        };
        let weak_active_uploads = Arc::downgrade(&self.bytestream_server.active_uploads);
        let mut active_uploads = self.bytestream_server.active_uploads.lock();
        let uuid = stream_state.uuid.clone();
        let Some(active_uploads_slot) = active_uploads.get_mut(&uuid) else {
            error!(
                err = "Failed to find active upload. This should never happen.",
                uuid = ?uuid,
            );
            return;
        };
        let timeout = self.timeout;
        active_uploads_slot.1 = Some(IdleStream {
            stream_state,
            _timeout_stream_drop_guard: spawn!("bytestream_idle_stream_timeout", async move {
                sleep(timeout).await;
                if let Some(active_uploads) = weak_active_uploads.upgrade() {
                    let mut active_uploads = active_uploads.lock();
                    info!(msg = "Removing idle stream", uuid = ?uuid);
                    active_uploads.remove(&uuid);
                }
            }),
        });
    }
}

/// Represents a stream that is in the "idle" state. this means it is not currently being used
/// by a client. If it is not used within a certain amount of time it will be removed from the
/// `active_uploads` map automatically.
#[derive(Debug)]
struct IdleStream {
    stream_state: StreamState,
    _timeout_stream_drop_guard: JoinHandleDropGuard<()>,
}

impl IdleStream {
    fn into_active_stream(
        self,
        bytes_received: Arc<AtomicU64>,
        bytestream_server: &ByteStreamServer,
        timeout: Duration,
    ) -> ActiveStreamGuard<'_> {
        ActiveStreamGuard {
            stream_state: Some(self.stream_state),
            bytes_received,
            bytestream_server,
            timeout,
        }
    }
}

type BytesWrittenAndIdleStream = (Arc<AtomicU64>, Option<IdleStream>);

#[derive(Debug)]
struct StoreConfig {
    cas_store: Store,
    max_bytes_per_stream: usize,
    persist_stream_on_disconnect_timeout: Duration,
}

#[derive(Debug)]
pub struct ByteStreamServer {
    active_uploads: Arc<Mutex<HashMap<String, BytesWrittenAndIdleStream>>>,
    max_decoding_message_size: usize,
    stores: HashMap<String, StoreConfig>,
}

impl ByteStreamServer {
    pub fn new(
        configs: &[WithInstanceName<ByteStreamConfig>],
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut stores = HashMap::with_capacity(configs.len());
        let mut max_decoding_message_sizes = Vec::new();

        for config in configs {
            let cas_store = store_manager.get_store(&config.cas_store).ok_or_else(|| {
                make_input_err!("'cas_store': '{}' does not exist", config.cas_store)
            })?;
            let max_bytes_per_stream = if config.max_bytes_per_stream == 0 {
                DEFAULT_MAX_BYTES_PER_STREAM
            } else {
                config.max_bytes_per_stream
            };
            max_decoding_message_sizes.push(config.max_decoding_message_size);
            let persist_stream_on_disconnect_timeout =
                if config.persist_stream_on_disconnect_timeout == 0 {
                    DEFAULT_PERSIST_STREAM_ON_DISCONNECT_TIMEOUT
                } else {
                    Duration::from_secs(config.persist_stream_on_disconnect_timeout as u64)
                };
            stores.insert(
                config.instance_name.clone(),
                StoreConfig {
                    cas_store,
                    max_bytes_per_stream,
                    persist_stream_on_disconnect_timeout,
                },
            );
        }

        let max_decoding_message_size = max_decoding_message_sizes
            .iter()
            .max()
            .copied()
            .unwrap_or(0);
        let max_decoding_message_size = if max_decoding_message_size == 0 {
            DEFAULT_MAX_DECODING_MESSAGE_SIZE
        } else {
            max_decoding_message_size
        };
        let unique_sizes: std::collections::HashSet<_> =
            max_decoding_message_sizes.into_iter().collect();
        if unique_sizes.len() > 1 {
            info!(
                "Multiple `max_decoding_message_size` values configured across ByteStream stores. Using maximum value: {max_decoding_message_size}"
            );
        }

        Ok(Self {
            active_uploads: Arc::new(Mutex::new(HashMap::new())),
            max_decoding_message_size,
            stores,
        })
    }

    pub fn into_service(self) -> Server<Self> {
        let max_decoding_message_size = self.max_decoding_message_size;
        Server::new(self).max_decoding_message_size(max_decoding_message_size)
    }

    fn get_store_config(&self, instance_name: &str) -> Result<&StoreConfig, Error> {
        self.stores.get(instance_name).ok_or_else(|| {
            make_input_err!("'instance_name' not configured for '{}'", instance_name)
        })
    }

    fn create_or_join_upload_stream(
        &self,
        uuid: String,
        digest: DigestInfo,
        store: Store,
        timeout: Duration,
    ) -> Result<ActiveStreamGuard<'_>, Error> {
        let (uuid, bytes_received) = match self.active_uploads.lock().entry(uuid) {
            Entry::Occupied(mut entry) => {
                let maybe_idle_stream = entry.get_mut();
                let Some(idle_stream) = maybe_idle_stream.1.take() else {
                    return Err(make_input_err!("Cannot upload same UUID simultaneously"));
                };
                let bytes_received = maybe_idle_stream.0.clone();
                info!(msg = "Joining existing stream", entry = ?entry.key());
                return Ok(idle_stream.into_active_stream(bytes_received, self, timeout));
            }
            Entry::Vacant(entry) => {
                let bytes_received = Arc::new(AtomicU64::new(0));
                let uuid = entry.key().clone();
                // Our stream is "in use" if the key is in the map, but the value is None.
                entry.insert((bytes_received.clone(), None));
                (uuid, bytes_received)
            }
        };

        // Important: Do not return an error from this point onwards without
        // removing the entry from the map, otherwise that UUID becomes
        // unusable.

        let (tx, rx) = make_buf_channel_pair();
        let store_update_fut = Box::pin(async move {
            // We need to wrap `Store::update()` in a another future because we need to capture
            // `store` to ensure its lifetime follows the future and not the caller.
            store
                // Bytestream always uses digest size as the actual byte size.
                .update(digest, rx, UploadSizeInfo::ExactSize(digest.size_bytes()))
                .await
        });
        Ok(ActiveStreamGuard {
            stream_state: Some(StreamState {
                uuid,
                tx,
                store_update_fut,
            }),
            bytes_received,
            bytestream_server: self,
            timeout,
        })
    }

    async fn inner_read(
        &self,
        digest: DigestInfo,
        max_bytes_per_stream: usize,
        read_request: ReadRequest,
        store: Store,
    ) -> Result<impl Stream<Item = Result<ReadResponse, Status>> + Send + use<>, Error> {
        struct ReaderState {
            max_bytes_per_stream: usize,
            rx: DropCloserReadHalf,
            maybe_get_part_result: Option<Result<(), Error>>,
            get_part_fut: Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>,
        }

        let read_limit = u64::try_from(read_request.read_limit)
            .err_tip(|| "Could not convert read_limit to u64")?;

        let (tx, rx) = make_buf_channel_pair();

        let read_limit = if read_limit != 0 {
            Some(read_limit)
        } else {
            None
        };

        // This allows us to call a destructor when the the object is dropped.
        let state = Some(ReaderState {
            rx,
            max_bytes_per_stream,
            maybe_get_part_result: None,
            get_part_fut: Box::pin(async move {
                store
                    .get_part(
                        digest,
                        tx,
                        u64::try_from(read_request.read_offset)
                            .err_tip(|| "Could not convert read_offset to u64")?,
                        read_limit,
                    )
                    .await
            }),
        });

        let read_stream_span = error_span!("read_stream");

        Ok(Box::pin(unfold(state, move |state| {
            async {
            let mut state = state?; // If None our stream is done.
            let mut response = ReadResponse::default();
            {
                let consume_fut = state.rx.consume(Some(state.max_bytes_per_stream));
                tokio::pin!(consume_fut);
                loop {
                    tokio::select! {
                        read_result = &mut consume_fut => {
                            match read_result {
                                Ok(bytes) => {
                                    if bytes.is_empty() {
                                        // EOF.
                                        return None;
                                    }
                                    if bytes.len() > state.max_bytes_per_stream {
                                        let err = make_err!(Code::Internal, "Returned store size was larger than read size");
                                        return Some((Err(err.into()), None));
                                    }
                                    response.data = bytes;
                                    trace!(response = ?response);
                                    debug!(response.data = format!("<redacted len({})>", response.data.len()));
                                    break;
                                }
                                Err(mut e) => {
                                    // We may need to propagate the error from reading the data through first.
                                    // For example, the NotFound error will come through `get_part_fut`, and
                                    // will not be present in `e`, but we need to ensure we pass NotFound error
                                    // code or the client won't know why it failed.
                                    let get_part_result = if let Some(result) = state.maybe_get_part_result {
                                        result
                                    } else {
                                        // This should never be `future::pending()` if maybe_get_part_result is
                                        // not set.
                                        state.get_part_fut.await
                                    };
                                    if let Err(err) = get_part_result {
                                        e = err.merge(e);
                                    }
                                    if e.code == Code::NotFound {
                                        // Trim the error code. Not Found is quite common and we don't want to send a large
                                        // error (debug) message for something that is common. We resize to just the last
                                        // message as it will be the most relevant.
                                        e.messages.truncate(1);
                                    }
                                    error!(response = ?e);
                                    return Some((Err(e.into()), None))
                                }
                            }
                        },
                        result = &mut state.get_part_fut => {
                            state.maybe_get_part_result = Some(result);
                            // It is non-deterministic on which future will finish in what order.
                            // It is also possible that the `state.rx.consume()` call above may not be able to
                            // respond even though the publishing future is done.
                            // Because of this we set the writing future to pending so it never finishes.
                            // The `state.rx.consume()` future will eventually finish and return either the
                            // data or an error.
                            // An EOF will terminate the `state.rx.consume()` future, but we are also protected
                            // because we are dropping the writing future, it will drop the `tx` channel
                            // which will eventually propagate an error to the `state.rx.consume()` future if
                            // the EOF was not sent due to some other error.
                            state.get_part_fut = Box::pin(pending());
                        },
                    }
                }
            }
            Some((Ok(response), Some(state)))
        }.instrument(read_stream_span.clone())
        })))
    }

    // We instrument tracing here as well as below because `stream` has a hash on it
    // that is extracted from the first stream message. If we only implemented it below
    // we would not have the hash available to us.
    #[instrument(
        ret(level = Level::DEBUG),
        level = Level::ERROR,
        skip(self),
        fields(stream.first_msg = "<redacted>")
    )]
    async fn inner_write(
        &self,
        digest: DigestInfo,
        store: Store,
        stream: WriteRequestStreamWrapper<impl Stream<Item = Result<WriteRequest, Status>> + Unpin>,
        timeout: Duration,
    ) -> Result<Response<WriteResponse>, Error> {
        async fn process_client_stream(
            mut stream: WriteRequestStreamWrapper<
                impl Stream<Item = Result<WriteRequest, Status>> + Unpin,
            >,
            tx: &mut DropCloserWriteHalf,
            outer_bytes_received: &Arc<AtomicU64>,
            expected_size: u64,
        ) -> Result<(), Error> {
            loop {
                let write_request = match stream.next().await {
                    // Code path for when client tries to gracefully close the stream.
                    // If this happens it means there's a problem with the data sent,
                    // because we always close the stream from our end before this point
                    // by counting the number of bytes sent from the client. If they send
                    // less than the amount they said they were going to send and then
                    // close the stream, we know there's a problem.
                    None => {
                        return Err(make_input_err!(
                            "Client closed stream before sending all data"
                        ));
                    }
                    // Code path for client stream error. Probably client disconnect.
                    Some(Err(err)) => return Err(err),
                    // Code path for received chunk of data.
                    Some(Ok(write_request)) => write_request,
                };

                if write_request.write_offset < 0 {
                    return Err(make_input_err!(
                        "Invalid negative write offset in write request: {}",
                        write_request.write_offset
                    ));
                }
                let write_offset = write_request.write_offset as u64;

                // If we get duplicate data because a client didn't know where
                // it left off from, then we can simply skip it.
                let data = if write_offset < tx.get_bytes_written() {
                    if (write_offset + write_request.data.len() as u64) < tx.get_bytes_written() {
                        if write_request.finish_write {
                            return Err(make_input_err!(
                                "Resumed stream finished at {} bytes when we already received {} bytes.",
                                write_offset + write_request.data.len() as u64,
                                tx.get_bytes_written()
                            ));
                        }
                        continue;
                    }
                    write_request
                        .data
                        .slice((tx.get_bytes_written() - write_offset) as usize..)
                } else {
                    if write_offset != tx.get_bytes_written() {
                        return Err(make_input_err!(
                            "Received out of order data. Got {}, expected {}",
                            write_offset,
                            tx.get_bytes_written()
                        ));
                    }
                    write_request.data
                };

                // Do not process EOF or weird stuff will happen.
                if !data.is_empty() {
                    // We also need to process the possible EOF branch, so we can't early return.
                    if let Err(mut err) = tx.send(data).await {
                        err.code = Code::Internal;
                        return Err(err);
                    }
                    outer_bytes_received.store(tx.get_bytes_written(), Ordering::Release);
                }

                if expected_size < tx.get_bytes_written() {
                    return Err(make_input_err!("Received more bytes than expected"));
                }
                if write_request.finish_write {
                    // Gracefully close our stream.
                    tx.send_eof()
                        .err_tip(|| "Failed to send EOF in ByteStream::write")?;
                    return Ok(());
                }
                // Continue.
            }
            // Unreachable.
        }

        let uuid = stream
            .resource_info
            .uuid
            .as_ref()
            .ok_or_else(|| make_input_err!("UUID must be set if writing data"))?
            .to_string();
        let mut active_stream_guard =
            self.create_or_join_upload_stream(uuid, digest, store, timeout)?;
        let expected_size = stream.resource_info.expected_size as u64;

        let active_stream = active_stream_guard.stream_state.as_mut().unwrap();
        try_join!(
            process_client_stream(
                stream,
                &mut active_stream.tx,
                &active_stream_guard.bytes_received,
                expected_size
            ),
            (&mut active_stream.store_update_fut)
                .map_err(|err| { err.append("Error updating inner store") })
        )?;

        // Close our guard and consider the stream no longer active.
        active_stream_guard.graceful_finish();

        Ok(Response::new(WriteResponse {
            committed_size: expected_size as i64,
        }))
    }

    async fn inner_query_write_status(
        &self,
        query_request: &QueryWriteStatusRequest,
    ) -> Result<Response<QueryWriteStatusResponse>, Error> {
        let mut resource_info = ResourceInfo::new(&query_request.resource_name, true)?;

        let store_config = self.get_store_config(resource_info.instance_name.as_ref())?;
        let store_clone = store_config.cas_store.clone();

        let digest = DigestInfo::try_new(resource_info.hash.as_ref(), resource_info.expected_size)?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        if let Some(grpc_store) = store_clone.downcast_ref::<GrpcStore>(Some(digest.into())) {
            return grpc_store
                .query_write_status(Request::new(query_request.clone()))
                .await;
        }

        let uuid = resource_info
            .uuid
            .take()
            .ok_or_else(|| make_input_err!("UUID must be set if querying write status"))?;

        {
            let active_uploads = self.active_uploads.lock();
            if let Some((received_bytes, _maybe_idle_stream)) = active_uploads.get(uuid.as_ref()) {
                return Ok(Response::new(QueryWriteStatusResponse {
                    committed_size: received_bytes.load(Ordering::Acquire) as i64,
                    // If we are in the active_uploads map, but the value is None,
                    // it means the stream is not complete.
                    complete: false,
                }));
            }
        }

        let has_fut = store_clone.has(digest);
        let Some(item_size) = has_fut.await.err_tip(|| "Failed to call .has() on store")? else {
            // We lie here and say that the stream needs to start over, even though
            // it was never started. This can happen when the client disconnects
            // before sending the first payload, but the client thinks it did send
            // the payload.
            return Ok(Response::new(QueryWriteStatusResponse {
                committed_size: 0,
                complete: false,
            }));
        };
        Ok(Response::new(QueryWriteStatusResponse {
            committed_size: item_size as i64,
            complete: true,
        }))
    }
}

#[tonic::async_trait]
impl ByteStream for ByteStreamServer {
    type ReadStream = ReadStream;

    #[instrument(
        err,
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn read(
        &self,
        grpc_request: Request<ReadRequest>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        let read_request = grpc_request.into_inner();
        let resource_name = read_request.resource_name.clone();
        let resource_info = ResourceInfo::new(&resource_name, false)?;
        let instance_name = resource_info.instance_name.as_ref();

        let store_config = self.get_store_config(instance_name)?;
        let store = store_config.cas_store.clone();
        let max_bytes_per_stream = store_config.max_bytes_per_stream;

        let digest = DigestInfo::try_new(resource_info.hash.as_ref(), resource_info.expected_size)?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        if let Some(grpc_store) = store.downcast_ref::<GrpcStore>(Some(digest.into())) {
            let stream = Box::pin(grpc_store.read(Request::new(read_request)).await?);
            return Ok(Response::new(stream));
        }

        let digest_function = resource_info.digest_function.as_deref().map_or_else(
            || Ok(default_digest_hasher_func()),
            DigestHasherFunc::try_from,
        )?;

        let resp = self
            .inner_read(digest, max_bytes_per_stream, read_request, store)
            .instrument(error_span!("bytestream_read"))
            .with_context(
                make_ctx_for_hash_func(digest_function).err_tip(|| "In BytestreamServer::read")?,
            )
            .await
            .err_tip(|| "In ByteStreamServer::read")
            .map(|stream| -> Response<Self::ReadStream> { Response::new(Box::pin(stream)) })
            .map_err(Into::into);

        if resp.is_ok() {
            debug!(return = "Ok(<stream>)");
        }

        resp
    }

    #[instrument(
        err,
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn write(
        &self,
        grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        let request = grpc_request.into_inner();
        let stream = WriteRequestStreamWrapper::from(request)
            .await
            .err_tip(|| "Could not unwrap first stream message")
            .map_err(Into::<Status>::into)?;

        let instance_name = stream.resource_info.instance_name.clone();

        let store_config = self.get_store_config(&instance_name)?;
        let store = store_config.cas_store.clone();
        let timeout = store_config.persist_stream_on_disconnect_timeout;

        let digest = DigestInfo::try_new(
            &stream.resource_info.hash,
            stream.resource_info.expected_size,
        )
        .err_tip(|| "Invalid digest input in ByteStream::write")?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        if let Some(grpc_store) = store.downcast_ref::<GrpcStore>(Some(digest.into())) {
            let resp = grpc_store.write(stream).await.map_err(Into::into);
            return resp;
        }

        let digest_function = stream
            .resource_info
            .digest_function
            .as_deref()
            .map_or_else(
                || Ok(default_digest_hasher_func()),
                DigestHasherFunc::try_from,
            )?;

        self.inner_write(digest, store, stream, timeout)
            .instrument(error_span!("bytestream_write"))
            .with_context(
                make_ctx_for_hash_func(digest_function).err_tip(|| "In BytestreamServer::write")?,
            )
            .await
            .err_tip(|| "In ByteStreamServer::write")
            .map_err(Into::into)
    }

    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn query_write_status(
        &self,
        grpc_request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        let request = grpc_request.into_inner();
        self.inner_query_write_status(&request)
            .await
            .err_tip(|| "Failed on query_write_status() command")
            .map_err(Into::into)
    }
}
