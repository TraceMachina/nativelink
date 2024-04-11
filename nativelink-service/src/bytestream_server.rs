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

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::{pending, BoxFuture};
use futures::stream::unfold;
use futures::{try_join, Future, Stream, TryFutureExt};
use nativelink_config::cas_server::ByteStreamConfig;
use nativelink_error::{make_err, make_input_err, Code, Error, ResultExt};
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
    make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::proto_stream_utils::WriteRequestStreamWrapper;
use nativelink_util::resource_info::ResourceInfo;
use nativelink_util::store_trait::{Store, UploadSizeInfo};
use parking_lot::Mutex;
use tokio::task::AbortHandle;
use tokio::time::sleep;
use tonic::{Request, Response, Status, Streaming};
use tracing::{enabled, error, info, Level};

/// If this value changes update the documentation in the config definition.
const DEFAULT_PERSIST_STREAM_ON_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// If this value changes update the documentation in the config definition.
const DEFAULT_MAX_BYTES_PER_STREAM: usize = 64 * 1024;

type ReadStream = Pin<Box<dyn Stream<Item = Result<ReadResponse, Status>> + Send + 'static>>;
type StoreUpdateFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'static>>;

struct StreamState {
    uuid: String,
    tx: DropCloserWriteHalf,
    store_update_fut: StoreUpdateFuture,
}

impl Debug for StreamState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
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
}

impl<'a> ActiveStreamGuard<'a> {
    /// Consumes the guard. The stream will be considered "finished", will
    /// remove it from the active_uploads.
    fn graceful_finish(mut self) {
        let stream_state = self.stream_state.take().unwrap();
        self.bytestream_server
            .active_uploads
            .lock()
            .remove(&stream_state.uuid);
    }
}

impl<'a> Drop for ActiveStreamGuard<'a> {
    fn drop(&mut self) {
        let Some(stream_state) = self.stream_state.take() else {
            return; // If None it means we don't want it put back into an IdleStream.
        };
        let weak_active_uploads = Arc::downgrade(&self.bytestream_server.active_uploads);
        let mut active_uploads = self.bytestream_server.active_uploads.lock();
        let uuid = stream_state.uuid.clone();
        let Some(active_uploads_slot) = active_uploads.get_mut(&uuid) else {
            error!(
                "Failed to find active upload for UUID: {}. This should never happen.",
                uuid
            );
            return;
        };
        let sleep_fn = self.bytestream_server.sleep_fn.clone();
        active_uploads_slot.1 = Some(IdleStream {
            stream_state,
            abort_timeout_handle: tokio::spawn(async move {
                (*sleep_fn)().await;
                if let Some(active_uploads) = weak_active_uploads.upgrade() {
                    let mut active_uploads = active_uploads.lock();
                    info!("Removing idle stream {uuid}");
                    active_uploads.remove(&uuid);
                }
            })
            .abort_handle(),
        });
    }
}

/// Represents a stream that is in the "idle" state. this means it is not currently being used
/// by a client. If it is not used within a certain amount of time it will be removed from the
/// `active_uploads` map automatically.
#[derive(Debug)]
struct IdleStream {
    stream_state: StreamState,
    abort_timeout_handle: AbortHandle,
}

impl IdleStream {
    fn into_active_stream(
        self,
        bytes_received: Arc<AtomicU64>,
        bytestream_server: &ByteStreamServer,
    ) -> ActiveStreamGuard<'_> {
        self.abort_timeout_handle.abort();
        ActiveStreamGuard {
            stream_state: Some(self.stream_state),
            bytes_received,
            bytestream_server,
        }
    }
}

type BytesWrittenAndIdleStream = (Arc<AtomicU64>, Option<IdleStream>);
type SleepFn = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

pub struct ByteStreamServer {
    stores: HashMap<String, Arc<dyn Store>>,
    // Max number of bytes to send on each grpc stream chunk.
    max_bytes_per_stream: usize,
    active_uploads: Arc<Mutex<HashMap<String, BytesWrittenAndIdleStream>>>,
    sleep_fn: SleepFn,
}

impl ByteStreamServer {
    pub fn new(config: &ByteStreamConfig, store_manager: &StoreManager) -> Result<Self, Error> {
        let mut persist_stream_on_disconnect_timeout =
            Duration::from_secs(config.persist_stream_on_disconnect_timeout as u64);
        if config.persist_stream_on_disconnect_timeout == 0 {
            persist_stream_on_disconnect_timeout = DEFAULT_PERSIST_STREAM_ON_DISCONNECT_TIMEOUT;
        }
        Self::new_with_sleep_fn(
            config,
            store_manager,
            Arc::new(move || Box::pin(sleep(persist_stream_on_disconnect_timeout))),
        )
    }

    pub fn new_with_sleep_fn(
        config: &ByteStreamConfig,
        store_manager: &StoreManager,
        sleep_fn: SleepFn,
    ) -> Result<Self, Error> {
        let mut stores = HashMap::with_capacity(config.cas_stores.len());
        for (instance_name, store_name) in &config.cas_stores {
            let store = store_manager
                .get_store(store_name)
                .ok_or_else(|| make_input_err!("'cas_store': '{}' does not exist", store_name))?;
            stores.insert(instance_name.to_string(), store);
        }
        let max_bytes_per_stream = if config.max_bytes_per_stream == 0 {
            DEFAULT_MAX_BYTES_PER_STREAM
        } else {
            config.max_bytes_per_stream
        };
        Ok(ByteStreamServer {
            stores,
            max_bytes_per_stream,
            active_uploads: Arc::new(Mutex::new(HashMap::new())),
            sleep_fn,
        })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    fn create_or_join_upload_stream(
        &self,
        uuid: String,
        store: Arc<dyn Store>,
        digest: DigestInfo,
    ) -> Result<ActiveStreamGuard<'_>, Error> {
        let (uuid, bytes_received) = match self.active_uploads.lock().entry(uuid) {
            Entry::Occupied(mut entry) => {
                let maybe_idle_stream = entry.get_mut();
                let Some(idle_stream) = maybe_idle_stream.1.take() else {
                    return Err(make_input_err!("Cannot upload same UUID simultaneously"));
                };
                let bytes_received = maybe_idle_stream.0.clone();
                info!("Joining existing stream {}", entry.key());
                return Ok(idle_stream.into_active_stream(bytes_received, self));
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
            // `store` to ensure it's lifetime follows the future and not the caller.
            Pin::new(store.as_ref())
                // Bytestream always uses digest size as the actual byte size.
                .update(
                    digest,
                    rx,
                    UploadSizeInfo::ExactSize(
                        usize::try_from(digest.size_bytes).err_tip(|| "Invalid digest size")?,
                    ),
                )
                .await
        });
        Ok(ActiveStreamGuard {
            stream_state: Some(StreamState {
                tx,
                store_update_fut,
                uuid,
            }),
            bytes_received,
            bytestream_server: self,
        })
    }

    async fn inner_read(
        &self,
        grpc_request: Request<ReadRequest>,
    ) -> Result<Response<ReadStream>, Error> {
        let read_request = grpc_request.into_inner();

        let read_limit = usize::try_from(read_request.read_limit)
            .err_tip(|| "read_limit has is not convertible to usize")?;
        let resource_info = ResourceInfo::new(&read_request.resource_name, false)?;
        let instance_name = resource_info.instance_name;
        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();

        let digest = DigestInfo::try_new(resource_info.hash, resource_info.expected_size)?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        let any_store = store.inner_store(Some(digest)).as_any();
        if let Some(grpc_store) = any_store.downcast_ref::<GrpcStore>() {
            let stream = grpc_store.read(Request::new(read_request)).await?;
            return Ok(Response::new(Box::pin(stream)));
        }

        let (tx, rx) = make_buf_channel_pair();

        struct ReaderState {
            max_bytes_per_stream: usize,
            rx: DropCloserReadHalf,
            maybe_get_part_result: Option<Result<(), Error>>,
            get_part_fut: Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>,
        }

        let read_limit = if read_limit != 0 {
            Some(read_limit)
        } else {
            None
        };

        // This allows us to call a destructor when the the object is dropped.
        let state = Some(ReaderState {
            rx,
            max_bytes_per_stream: self.max_bytes_per_stream,
            maybe_get_part_result: None,
            get_part_fut: Box::pin(async move {
                store
                    .get_part_arc(digest, tx, read_request.read_offset as usize, read_limit)
                    .await
            }),
        });

        Ok(Response::new(Box::pin(unfold(state, move |state| async {
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
                                        return Some((Ok(response), None));
                                    }
                                    if bytes.len() > state.max_bytes_per_stream {
                                        let err = make_err!(Code::Internal, "Returned store size was larger than read size");
                                        return Some((Err(err.into()), None));
                                    }
                                    response.data = bytes;
                                    info!("\x1b[0;31mBytestream Read Chunk Resp\x1b[0m: {:?}", response);
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
                                    info!("\x1b[0;31mBytestream Read Chunk Resp\x1b[0m: Error {:?}", e);
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
        }))))
    }

    async fn inner_write(
        &self,
        mut stream: WriteRequestStreamWrapper<Streaming<WriteRequest>, Status>,
    ) -> Result<Response<WriteResponse>, Error> {
        let instance_name = &stream.instance_name;
        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();

        let digest = DigestInfo::try_new(&stream.hash, stream.expected_size)
            .err_tip(|| "Invalid digest input in ByteStream::write")?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        let any_store = store.inner_store(Some(digest)).as_any();
        if let Some(grpc_store) = any_store.downcast_ref::<GrpcStore>() {
            return grpc_store.write(stream).await;
        }

        let uuid = stream
            .uuid
            .take()
            .ok_or_else(|| make_input_err!("UUID must be set if writing data"))?;
        let mut active_stream_guard = self.create_or_join_upload_stream(uuid, store, digest)?;
        let expected_size = stream.expected_size as u64;

        async fn process_client_stream(
            mut stream: WriteRequestStreamWrapper<Streaming<WriteRequest>, Status>,
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
                        ))
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

        let store_clone = self
            .stores
            .get(resource_info.instance_name)
            .err_tip(|| {
                format!(
                    "'instance_name' not configured for '{}'",
                    &resource_info.instance_name
                )
            })?
            .clone();

        let digest = DigestInfo::try_new(resource_info.hash, resource_info.expected_size)?;

        // If we are a GrpcStore we shortcut here, as this is a special store.
        let any_store = store_clone.inner_store(Some(digest)).as_any();
        if let Some(grpc_store) = any_store.downcast_ref::<GrpcStore>() {
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
            if let Some((received_bytes, _maybe_idle_stream)) = active_uploads.get(uuid) {
                return Ok(Response::new(QueryWriteStatusResponse {
                    committed_size: received_bytes.load(Ordering::Acquire) as i64,
                    // If we are in the active_uploads map, but the value is None,
                    // it means the stream is not complete.
                    complete: false,
                }));
            }
        }

        let has_fut = Pin::new(store_clone.as_ref()).has(digest);
        let Some(item_size) = has_fut.await.err_tip(|| "Failed to call .has() on store")? else {
            return Err(make_err!(Code::NotFound, "{}", "not found"));
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
    async fn read(
        &self,
        grpc_request: Request<ReadRequest>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        info!("\x1b[0;31mRead Req\x1b[0m: {:?}", grpc_request.get_ref());
        let now = Instant::now();
        let resp = self
            .inner_read(grpc_request)
            .await
            .err_tip(|| "Failed on read() command")
            .map_err(|e| e.into());
        let d = now.elapsed().as_secs_f32();
        if let Err(err) = resp.as_ref() {
            error!("\x1b[0;31mRead Resp\x1b[0m: {} {:?}", d, err);
        } else {
            info!("\x1b[0;31mRead Resp\x1b[0m: {}", d);
        }
        resp
    }

    async fn write(
        &self,
        grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        let now = Instant::now();
        let stream = WriteRequestStreamWrapper::from(grpc_request.into_inner())
            .await
            .err_tip(|| "Could not unwrap first stream message")
            .map_err(Into::<Status>::into)?;
        let hash = if enabled!(Level::DEBUG) {
            Some(stream.hash.clone())
        } else {
            None
        };

        info!("\x1b[0;31mWrite Req\x1b[0m: {:?}", hash);

        let resp = self
            .inner_write(stream)
            .await
            .err_tip(|| "In ByteStreamServer::write()")
            .map_err(|e| e.into());

        let d = now.elapsed().as_secs_f32();
        if let Err(err) = resp.as_ref() {
            error!("\x1b[0;31mWrite Resp\x1b[0m: {} {:?} {:?}", d, hash, err);
        } else {
            info!("\x1b[0;31mWrite Resp\x1b[0m: {} {:?}", d, hash);
        }
        resp
    }

    async fn query_write_status(
        &self,
        grpc_request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        let now = Instant::now();
        let query_request = grpc_request.into_inner();

        let resp = self
            .inner_query_write_status(&query_request)
            .await
            .err_tip(|| "Failed on query_write_status() command")
            .map_err(|e| e.into());

        let d = now.elapsed().as_secs_f32();
        if resp.is_err() {
            error!("\x1b[0;31mQuery Req\x1b[0m: {:?}", query_request);
            error!("\x1b[0;31mQuery Resp\x1b[0m: {} {:?}", d, resp);
        } else {
            info!("\x1b[0;31mQuery Req\x1b[0m: {:?}", query_request);
            info!("\x1b[0;31mQuery Resp\x1b[0m: {} {:?}", d, resp);
        }
        resp
    }
}
