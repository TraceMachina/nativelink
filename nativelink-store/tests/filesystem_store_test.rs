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

use core::fmt::{Debug, Formatter};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::time::Duration;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::{Arc, LazyLock};

use async_lock::RwLock;
use bytes::Bytes;
use futures::executor::block_on;
use futures::task::Poll;
use futures::{Future, FutureExt, poll};
use nativelink_config::stores::{EvictionPolicy, FilesystemSpec};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_store::filesystem_store::{
    DIGEST_FOLDER, EncodedFilePath, FileEntry, FileEntryImpl, FileType, FilesystemStore,
    STR_FOLDER, key_from_file,
};
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::{DigestInfo, fs};
use nativelink_util::evicting_map::LenEntry;
use nativelink_util::store_trait::{Store, StoreKey, StoreLike, UploadSizeInfo};
use nativelink_util::{background_spawn, spawn};
use opentelemetry::context::{Context, FutureExt as OtelFutureExt};
use parking_lot::Mutex;
use pretty_assertions::assert_eq;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, Take};
use tokio::sync::{Barrier, Semaphore};
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReadDirStream;
use tracing::Instrument;

const VALID_HASH: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

fn make_random_data(sz: usize) -> Vec<u8> {
    let mut value = vec![0u8; sz];
    let mut rng = SmallRng::seed_from_u64(1);
    rng.fill(&mut value[..]);
    value
}

trait FileEntryHooks {
    fn on_make_and_open(
        _encoded_file_path: &EncodedFilePath,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        core::future::ready(Ok(()))
    }
    fn on_unref<Fe: FileEntry>(_entry: &Fe) {}
    fn on_drop<Fe: FileEntry>(_entry: &Fe) {}
}

struct TestFileEntry<Hooks: FileEntryHooks + 'static + Sync + Send> {
    inner: Option<FileEntryImpl>,
    _phantom: PhantomData<Hooks>,
}

impl<Hooks: FileEntryHooks + 'static + Sync + Send> Debug for TestFileEntry<Hooks> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), core::fmt::Error> {
        f.debug_struct("TestFileEntry")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<Hooks: FileEntryHooks + 'static + Sync + Send> FileEntry for TestFileEntry<Hooks> {
    fn create(data_size: u64, block_size: u64, encoded_file_path: RwLock<EncodedFilePath>) -> Self {
        Self {
            inner: Some(FileEntryImpl::create(
                data_size,
                block_size,
                encoded_file_path,
            )),
            _phantom: PhantomData,
        }
    }

    async fn make_and_open_file(
        block_size: u64,
        encoded_file_path: EncodedFilePath,
    ) -> Result<(Self, fs::FileSlot, OsString), Error> {
        Hooks::on_make_and_open(&encoded_file_path).await?;
        let (inner, file_slot, path) =
            FileEntryImpl::make_and_open_file(block_size, encoded_file_path).await?;
        Ok((
            Self {
                inner: Some(inner),
                _phantom: PhantomData,
            },
            file_slot,
            path,
        ))
    }

    fn data_size_mut(&mut self) -> &mut u64 {
        self.inner.as_mut().unwrap().data_size_mut()
    }

    fn size_on_disk(&self) -> u64 {
        self.inner.as_ref().unwrap().size_on_disk()
    }

    fn get_encoded_file_path(&self) -> &RwLock<EncodedFilePath> {
        self.inner.as_ref().unwrap().get_encoded_file_path()
    }

    async fn read_file_part(&self, offset: u64, length: u64) -> Result<Take<fs::FileSlot>, Error> {
        self.inner
            .as_ref()
            .unwrap()
            .read_file_part(offset, length)
            .await
    }

    async fn get_file_path_locked<
        T,
        Fut: Future<Output = Result<T, Error>> + Send,
        F: FnOnce(OsString) -> Fut + Send,
    >(
        &self,
        handler: F,
    ) -> Result<T, Error> {
        self.inner
            .as_ref()
            .unwrap()
            .get_file_path_locked(handler)
            .await
    }
}

impl<Hooks: FileEntryHooks + 'static + Sync + Send> LenEntry for TestFileEntry<Hooks> {
    fn len(&self) -> u64 {
        self.inner.as_ref().unwrap().len()
    }

    fn is_empty(&self) -> bool {
        self.inner.as_ref().unwrap().is_empty()
    }

    async fn unref(&self) {
        Hooks::on_unref(self);
        self.inner.as_ref().unwrap().unref().await;
    }
}

impl<Hooks: FileEntryHooks + 'static + Sync + Send> Drop for TestFileEntry<Hooks> {
    fn drop(&mut self) {
        let mut inner = self.inner.take().unwrap();
        let shared_context = inner.get_shared_context_for_test();
        let current_context = Context::current();

        // We do this complicated bit here because tokio does not give a way to run a
        // command that will wait for all tasks and sub spawns to complete.
        // Sadly we need to rely on `active_drop_spawns` to hit zero to ensure that
        // all tasks have completed.
        let fut = async move {
            // Drop the FileEntryImpl in a controlled setting then wait for the
            // `active_drop_spawns` to hit zero.
            drop(inner);
            while shared_context.active_drop_spawns.load(Ordering::Acquire) > 0 {
                tokio::task::yield_now().await;
            }
        }
        .instrument(tracing::error_span!("test_file_entry_drop"))
        .with_context(current_context);

        #[expect(clippy::disallowed_methods, reason = "testing implementation")]
        let thread_handle = {
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                rt.block_on(fut);
            })
        };
        thread_handle.join().unwrap();
        // At this point we can guarantee our file drop spawn has completed.
        Hooks::on_drop(self);
    }
}

/// Get temporary path from either `TEST_TMPDIR` or best effort temp directory if
/// not set.
fn make_temp_path(data: &str) -> String {
    format!(
        "{}/{}/{}",
        env::var("TEST_TMPDIR").unwrap_or_else(|_| env::temp_dir().to_str().unwrap().to_string()),
        rand::rng().random::<u64>(),
        data
    )
}

async fn read_file_contents(file_name: &OsStr) -> Result<Vec<u8>, Error> {
    let mut file = fs::open_file(file_name, 0, u64::MAX)
        .await
        .err_tip(|| format!("Failed to open file: {}", file_name.display()))?;
    let mut data = vec![];
    file.read_to_end(&mut data)
        .await
        .err_tip(|| "Error reading file to end")?;
    Ok(data)
}

async fn wait_for_no_open_files() -> Result<(), Error> {
    let mut counter = 0;
    while fs::get_open_files_for_test() != 0 {
        sleep(Duration::from_millis(1)).await;
        counter += 1;
        if counter > 1000 {
            return Err(make_err!(
                Code::Internal,
                "Timed out waiting all files to close"
            ));
        }
    }
    Ok(())
}

/// Helper function to ensure there are no temporary or content files left.
async fn check_storage_dir_empty(storage_path: &str) -> Result<(), Error> {
    let (_permit, temp_dir_handle) = fs::read_dir(format!("{storage_path}/{DIGEST_FOLDER}"))
        .await
        .err_tip(|| "Failed opening temp directory")?
        .into_inner();

    let mut read_dir_stream = ReadDirStream::new(temp_dir_handle);

    if let Some(temp_dir_entry) = read_dir_stream.next().await {
        let path = temp_dir_entry?.path();
        panic!(
            "No files should exist in temp directory, found: {}",
            path.display()
        );
    }

    let (_permit, temp_dir_handle) = fs::read_dir(format!("{storage_path}/{STR_FOLDER}"))
        .await
        .err_tip(|| "Failed opening temp directory")?
        .into_inner();

    let mut read_dir_stream = ReadDirStream::new(temp_dir_handle);

    if let Some(temp_dir_entry) = read_dir_stream.next().await {
        let path = temp_dir_entry?.path();
        panic!(
            "No files should exist in temp directory, found: {}",
            path.display()
        );
    }
    Ok(())
}

const HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
const HASH2: &str = "0123456789abcdef000000000000000000020000000000000123456789abcdef";
const VALUE1: &str = "0123456789";
const VALUE2: &str = "9876543210";
const STRING_NAME: &str = "String_Filename";

#[nativelink_test]
async fn valid_results_after_shutdown_test() -> Result<(), Error> {
    let digest = DigestInfo::try_new(HASH1, VALUE1.len())?;
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");
    {
        let store = Store::new(
            FilesystemStore::<FileEntryImpl>::new(&FilesystemSpec {
                content_path: content_path.clone(),
                temp_path: temp_path.clone(),
                eviction_policy: None,
                block_size: 1,
                ..Default::default()
            })
            .await?,
        );
        // Insert dummy value into store.
        store.update_oneshot(digest, VALUE1.into()).await?;

        assert_eq!(
            store.has(digest).await,
            Ok(Some(VALUE1.len() as u64)),
            "Expected filesystem store to have hash: {}",
            HASH1
        );
    }
    {
        // With a new store ensure content is still readable (ie: restores from shutdown).
        let store = Box::pin(
            FilesystemStore::<FileEntryImpl>::new(&FilesystemSpec {
                content_path,
                temp_path,
                eviction_policy: None,
                ..Default::default()
            })
            .await?,
        );

        let key = StoreKey::Digest(digest);

        let content = store.get_part_unchunked(key, 0, None).await?;
        assert_eq!(content, VALUE1.as_bytes());
    }

    Ok(())
}

#[nativelink_test]
async fn temp_files_get_deleted_on_replace_test() -> Result<(), Error> {
    static DELETES_FINISHED: AtomicU32 = AtomicU32::new(0);
    struct LocalHooks {}
    impl FileEntryHooks for LocalHooks {
        fn on_drop<Fe: FileEntry>(_file_entry: &Fe) {
            DELETES_FINISHED.fetch_add(1, Ordering::Relaxed);
        }
    }

    let digest1 = DigestInfo::try_new(HASH1, VALUE1.len())?;
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let store = Box::pin(
        FilesystemStore::<TestFileEntry<LocalHooks>>::new(&FilesystemSpec {
            content_path: content_path.clone(),
            temp_path: temp_path.clone(),
            eviction_policy: Some(EvictionPolicy {
                max_count: 3,
                ..Default::default()
            }),
            ..Default::default()
        })
        .await?,
    );

    store.update_oneshot(digest1, VALUE1.into()).await?;

    let expected_file_name = OsString::from(format!("{content_path}/{DIGEST_FOLDER}/{digest1}"));
    {
        // Check to ensure our file exists where it should and content matches.
        let data = read_file_contents(&expected_file_name).await?;
        assert_eq!(
            &data[..],
            VALUE1.as_bytes(),
            "Expected file content to match"
        );
    }

    // Replace content.
    store.update_oneshot(digest1, VALUE2.into()).await?;

    {
        // Check to ensure our file now has new content.
        let data = read_file_contents(&expected_file_name).await?;
        assert_eq!(
            &data[..],
            VALUE2.as_bytes(),
            "Expected file content to match"
        );
    }

    loop {
        if DELETES_FINISHED.load(Ordering::Relaxed) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert!(logs_contain(
        "Spawned a filesystem_delete_file current_active_drop_spawns=1"
    ));
    assert!(logs_contain(
        "Dropped a filesystem_delete_file current_active_drop_spawns=0"
    ));

    check_storage_dir_empty(&temp_path).await
}

// This test ensures that if a file is overridden and an open stream to the file already
// exists, the open stream will continue to work properly and when the stream is done the
// temporary file (of the object that was deleted) is cleaned up.
#[nativelink_test]
async fn file_continues_to_stream_on_content_replace_test() -> Result<(), Error> {
    static DELETES_FINISHED: AtomicU32 = AtomicU32::new(0);
    struct LocalHooks {}
    impl FileEntryHooks for LocalHooks {
        fn on_drop<Fe: FileEntry>(_file_entry: &Fe) {
            DELETES_FINISHED.fetch_add(1, Ordering::Relaxed);
        }
    }

    let digest1 = DigestInfo::try_new(HASH1, VALUE1.len())?;
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let store = Arc::new(
        FilesystemStore::<TestFileEntry<LocalHooks>>::new(&FilesystemSpec {
            content_path: content_path.clone(),
            temp_path: temp_path.clone(),
            eviction_policy: Some(EvictionPolicy {
                max_count: 3,
                ..Default::default()
            }),
            block_size: 1,
            read_buffer_size: 1,
        })
        .await?,
    );

    // Insert data into store.
    store.update_oneshot(digest1, VALUE1.into()).await?;

    let (writer, mut reader) = make_buf_channel_pair();
    let store_clone = store.clone();
    let digest1_clone = digest1;
    background_spawn!(
        "file_continues_to_stream_on_content_replace_test_store_get",
        async move { store_clone.get(digest1_clone, writer).await.unwrap() },
    );

    {
        // Check to ensure our first byte has been received. The future should be stalled here.
        let first_byte = reader
            .consume(Some(1))
            .await
            .err_tip(|| "Error reading first byte")?;
        assert_eq!(
            first_byte[0],
            VALUE1.as_bytes()[0],
            "Expected first byte to match"
        );
    }

    // Replace content.
    store.update_oneshot(digest1, VALUE2.into()).await?;

    // Ensure we let any background tasks finish.
    tokio::task::yield_now().await;

    {
        // Now ensure we only have 1 file in our temp path - we know it is a digest.
        let (_permit, temp_dir_handle) = fs::read_dir(format!("{temp_path}/{DIGEST_FOLDER}"))
            .await
            .err_tip(|| "Failed opening temp directory")?
            .into_inner();
        let mut read_dir_stream = ReadDirStream::new(temp_dir_handle);
        let mut num_files = 0;
        while let Some(temp_dir_entry) = read_dir_stream.next().await {
            num_files += 1;
            let path = temp_dir_entry?.path();
            let data = read_file_contents(path.as_os_str()).await?;
            assert_eq!(
                &data[..],
                VALUE1.as_bytes(),
                "Expected file content to match"
            );
        }
        assert_eq!(
            num_files, 1,
            "There should only be one file in the temp directory"
        );
    }

    let remaining_file_data = reader
        .consume(Some(1024))
        .await
        .err_tip(|| "Error reading remaining bytes")?;

    assert_eq!(
        &remaining_file_data,
        &VALUE1.as_bytes()[1..],
        "Expected file content to match"
    );

    loop {
        if DELETES_FINISHED.load(Ordering::Relaxed) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }

    // Now ensure our temp file was cleaned up.
    check_storage_dir_empty(&temp_path).await
}

// Eviction has a different code path than a file replacement, so we check that if a
// file is evicted and has an open stream on it, it will stay alive and eventually
// get deleted.
#[nativelink_test]
async fn file_gets_cleans_up_on_cache_eviction() -> Result<(), Error> {
    static DELETES_FINISHED: AtomicU32 = AtomicU32::new(0);
    struct LocalHooks {}
    impl FileEntryHooks for LocalHooks {
        fn on_drop<Fe: FileEntry>(_file_entry: &Fe) {
            DELETES_FINISHED.fetch_add(1, Ordering::Relaxed);
        }
    }

    let digest1 = DigestInfo::try_new(HASH1, VALUE1.len())?;
    let digest2 = DigestInfo::try_new(HASH2, VALUE2.len())?;
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let store = Arc::new(
        FilesystemStore::<TestFileEntry<LocalHooks>>::new(&FilesystemSpec {
            content_path: content_path.clone(),
            temp_path: temp_path.clone(),
            eviction_policy: Some(EvictionPolicy {
                max_count: 1,
                ..Default::default()
            }),
            block_size: 1,
            read_buffer_size: 1,
        })
        .await?,
    );

    // Insert data into store.
    store.update_oneshot(digest1, VALUE1.into()).await.unwrap();

    let mut reader = {
        let (writer, reader) = make_buf_channel_pair();
        let store_clone = store.clone();
        background_spawn!(
            "file_gets_cleans_up_on_cache_eviction_store_get",
            async move { store_clone.get(digest1, writer).await.unwrap() },
        );
        reader
    };
    // Ensure we have received 1 byte in our buffer. This will ensure we have a reference to
    // our file open.
    assert!(reader.peek().await.is_ok(), "Could not peek into reader");

    // Insert new content. This will evict the old item.
    store.update_oneshot(digest2, VALUE2.into()).await?;

    // Ensure we let any background tasks finish.
    tokio::task::yield_now().await;

    {
        // Now ensure we only have 1 file in our temp path - we know it is a digest.
        let (_permit, temp_dir_handle) = fs::read_dir(format!("{temp_path}/{DIGEST_FOLDER}"))
            .await
            .err_tip(|| "Failed opening temp directory")?
            .into_inner();
        let mut read_dir_stream = ReadDirStream::new(temp_dir_handle);
        let mut num_files = 0;
        while let Some(temp_dir_entry) = read_dir_stream.next().await {
            num_files += 1;
            let path = temp_dir_entry?.path();
            let data = read_file_contents(path.as_os_str()).await?;
            assert_eq!(
                &data[..],
                VALUE1.as_bytes(),
                "Expected file content to match"
            );
        }
        assert_eq!(
            num_files, 1,
            "There should only be one file in the temp directory"
        );
    }

    let reader_data = reader
        .consume(Some(1024))
        .await
        .err_tip(|| "Error reading remaining bytes")?;

    assert_eq!(&reader_data, VALUE1, "Expected file content to match");

    loop {
        if DELETES_FINISHED.load(Ordering::Relaxed) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }

    // Now ensure our temp file was cleaned up.
    check_storage_dir_empty(&temp_path).await
}

// Test to ensure that if we are holding a reference to `FileEntry` and the contents are
// replaced, the `FileEntry` continues to use the old data.
// `FileEntry` file contents should be immutable for the lifetime of the object.
#[nativelink_test]
async fn digest_contents_replaced_continues_using_old_data() -> Result<(), Error> {
    let digest = DigestInfo::try_new(HASH1, VALUE1.len())?;

    let store = Box::pin(
        FilesystemStore::<FileEntryImpl>::new(&FilesystemSpec {
            content_path: make_temp_path("content_path"),
            temp_path: make_temp_path("temp_path"),
            eviction_policy: None,
            ..Default::default()
        })
        .await?,
    );
    // Insert data into store.
    store.update_oneshot(digest, VALUE1.into()).await?;
    let file_entry = store.get_file_entry_for_digest(&digest).await?;
    {
        // The file contents should equal our initial data.
        let mut reader = file_entry.read_file_part(0, u64::MAX).await?;
        let mut file_contents = String::new();
        reader.read_to_string(&mut file_contents).await?;
        assert_eq!(file_contents, VALUE1);
    }

    // Now replace the data.
    store.update_oneshot(digest, VALUE2.into()).await?;

    {
        // The file contents still equal our old data.
        let mut reader = file_entry.read_file_part(0, u64::MAX).await?;
        let mut file_contents = String::new();
        reader.read_to_string(&mut file_contents).await?;
        assert_eq!(file_contents, VALUE1);
    }

    Ok(())
}

#[nativelink_test]
async fn eviction_on_insert_calls_unref_once() -> Result<(), Error> {
    const SMALL_VALUE: &str = "01";
    const BIG_VALUE: &str = "0123";

    static UNREFED_DIGESTS: LazyLock<Mutex<Vec<StoreKey<'static>>>> =
        LazyLock::new(|| Mutex::new(Vec::new()));
    struct LocalHooks {}
    impl FileEntryHooks for LocalHooks {
        fn on_unref<Fe: FileEntry>(file_entry: &Fe) {
            block_on(file_entry.get_file_path_locked(move |path_str| async move {
                let path = Path::new(&path_str);
                let digest = key_from_file(
                    path.file_name().unwrap().to_str().unwrap(),
                    FileType::Digest,
                )
                .unwrap();
                UNREFED_DIGESTS.lock().push(digest.borrow().into_owned());
                Ok(())
            }))
            .unwrap();
        }
    }

    let small_digest = StoreKey::Digest(DigestInfo::try_new(HASH1, SMALL_VALUE.len())?);
    let big_digest = DigestInfo::try_new(HASH1, BIG_VALUE.len())?;

    let store = Box::pin(
        FilesystemStore::<TestFileEntry<LocalHooks>>::new(&FilesystemSpec {
            content_path: make_temp_path("content_path"),
            temp_path: make_temp_path("temp_path"),
            eviction_policy: Some(EvictionPolicy {
                max_bytes: 5,
                ..Default::default()
            }),
            block_size: 1,
            ..Default::default()
        })
        .await?,
    );
    // Insert data into store.
    store
        .update_oneshot(small_digest.borrow(), SMALL_VALUE.into())
        .await?;
    store.update_oneshot(big_digest, BIG_VALUE.into()).await?;

    {
        // Our first digest should have been unrefed exactly once.
        let unrefed_digests = UNREFED_DIGESTS.lock();
        assert_eq!(
            unrefed_digests.len(),
            1,
            "Expected exactly 1 unrefed digest"
        );
        assert_eq!(unrefed_digests[0], small_digest, "Expected digest to match");
    }

    Ok(())
}

#[nativelink_test]
async fn rename_on_insert_fails_due_to_filesystem_error_proper_cleanup_happens() -> Result<(), Error>
{
    const INITIAL_CONTENT: &str = "hello";

    async fn wait_for_temp_file<Fut: Future<Output = Result<(), Error>>, F: Fn() -> Fut>(
        temp_path: &str,
        yield_fn: F,
    ) -> Result<fs::DirEntry, Error> {
        loop {
            yield_fn().await?;
            // Now ensure we only have 1 file in our temp path - we know it is a digest.
            let (_permit, dir_handle) = fs::read_dir(format!("{temp_path}/{DIGEST_FOLDER}"))
                .await?
                .into_inner();
            let mut read_dir_stream = ReadDirStream::new(dir_handle);
            if let Some(dir_entry) = read_dir_stream.next().await {
                assert!(
                    read_dir_stream.next().await.is_none(),
                    "There should only be one file in temp directory"
                );
                let dir_entry = dir_entry?;
                {
                    // Some filesystems won't sync automatically, so force it.
                    let file_handle = fs::open_file(dir_entry.path().into_os_string(), 0, u64::MAX)
                        .await
                        .err_tip(|| "Failed to open temp file")?;
                    // We don't care if it fails, this is only best attempt.
                    drop(file_handle.get_ref().as_ref().sync_all().await);
                }
                // Ensure we have written to the file too. This ensures we have an open file handle.
                // Failing to do this may result in the file existing, but the `update_fut` not actually
                // sending data to it yet.
                if dir_entry.metadata().await?.len() >= INITIAL_CONTENT.len() as u64 {
                    return Ok(dir_entry);
                }
            }
        }
        // Unreachable.
    }

    static FILE_DELETED_BARRIER: LazyLock<Arc<Barrier>> =
        LazyLock::new(|| Arc::new(Barrier::new(2)));

    struct LocalHooks {}
    impl FileEntryHooks for LocalHooks {
        fn on_drop<Fe: FileEntry>(_file_entry: &Fe) {
            background_spawn!(
                "rename_on_insert_fails_due_to_filesystem_error_proper_cleanup_happens_local_hooks_on_drop",
                FILE_DELETED_BARRIER.wait()
            );
        }
    }

    let digest = DigestInfo::try_new(HASH1, VALUE1.len())?;

    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let store = Box::pin(
        FilesystemStore::<TestFileEntry<LocalHooks>>::new(&FilesystemSpec {
            content_path: content_path.clone(),
            temp_path: temp_path.clone(),
            eviction_policy: None,
            ..Default::default()
        })
        .await?,
    );

    let (mut tx, rx) = make_buf_channel_pair();
    let update_fut = Arc::new(async_lock::Mutex::new(store.update(
        digest,
        rx,
        UploadSizeInfo::MaxSize(100),
    )));
    // This will process as much of the future as it can before it needs to pause.
    // Our temp file will be created and opened and ready to have contents streamed
    // to it.
    assert_eq!(poll!(&mut *update_fut.lock().await)?, Poll::Pending);
    tx.send(INITIAL_CONTENT.into()).await?;

    // Now we extract that temp file that is generated.
    wait_for_temp_file(&temp_path, || {
        let update_fut_clone = update_fut.clone();
        async move {
            // This will ensure we yield to our future and other potential spawns.
            tokio::task::yield_now().await;
            assert_eq!(poll!(&mut *update_fut_clone.lock().await)?, Poll::Pending);
            Ok(())
        }
    })
    .await?;

    // Now make it impossible for the file to be moved into the final path.
    // This will trigger an error on `rename()`.
    fs::remove_dir_all(&content_path).await?;

    // Because send_eof() waits for shutdown of the rx side, we cannot just await in this thread.
    background_spawn!(
        "rename_on_insert_fails_due_to_filesystem_error_proper_cleanup_happens_send_eof",
        async move {
            tx.send_eof().unwrap();
        },
    );

    // Now finish waiting on update(). This should result in an error because we deleted our dest
    // folder.
    let update_result = &mut *update_fut.lock().await;
    assert!(
        update_result.await.is_err(),
        "Expected update to fail due to temp file being deleted before rename"
    );

    // Delete may happen on another thread, so wait for it.
    FILE_DELETED_BARRIER.wait().await;

    // Now it should have cleaned up its temp files.
    {
        check_storage_dir_empty(&temp_path).await?;
    }

    // Finally ensure that our entry is not in the store.
    assert_eq!(
        store.has(digest).await?,
        None,
        "Entry should not be in store"
    );
    Ok(())
}

#[nativelink_test]
async fn get_part_timeout_test() -> Result<(), Error> {
    let large_value = "x".repeat(1024);
    let digest = DigestInfo::try_new(HASH1, large_value.len())?;
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let store = Arc::new(
        FilesystemStore::<FileEntryImpl>::new_with_timeout_and_rename_fn(
            &FilesystemSpec {
                content_path: content_path.clone(),
                temp_path: temp_path.clone(),
                read_buffer_size: 1,
                ..Default::default()
            },
            |from, to| std::fs::rename(from, to),
        )
        .await?,
    );

    store
        .update_oneshot(digest, large_value.clone().into())
        .await?;

    let (writer, mut reader) = make_buf_channel_pair();
    let store_clone = store.clone();
    let digest_clone = digest;

    let _drop_guard = spawn!("get_part_timeout_test_get", async move {
        store_clone.get(digest_clone, writer).await
    });

    let file_data = reader
        .consume(Some(1024))
        .await
        .err_tip(|| "Error reading bytes")?;

    assert_eq!(
        &file_data,
        large_value.as_bytes(),
        "Expected file content to match"
    );

    Ok(())
}

#[nativelink_test]
async fn get_part_is_zero_digest() -> Result<(), Error> {
    let digest = DigestInfo::new(Sha256::new().finalize().into(), 0);
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let store = Arc::new(
        FilesystemStore::<FileEntryImpl>::new_with_timeout_and_rename_fn(
            &FilesystemSpec {
                content_path: content_path.clone(),
                temp_path: temp_path.clone(),
                read_buffer_size: 1,
                ..Default::default()
            },
            |from, to| std::fs::rename(from, to),
        )
        .await?,
    );

    let store_clone = store.clone();
    let (mut writer, mut reader) = make_buf_channel_pair();

    let _drop_guard = spawn!("get_part_is_zero_digest_get_part", async move {
        drop(
            store_clone
                .get_part(digest, &mut writer, 0, None)
                .await
                .err_tip(|| "Failed to get_part"),
        );
    });

    let file_data = reader
        .consume(Some(1024))
        .await
        .err_tip(|| "Error reading bytes")?;

    let empty_bytes = Bytes::new();
    assert_eq!(&file_data, &empty_bytes, "Expected file content to match");

    Ok(())
}

#[nativelink_test]
async fn has_with_results_on_zero_digests() -> Result<(), Error> {
    let digest = DigestInfo::new(Sha256::new().finalize().into(), 0);
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let store = Arc::new(
        FilesystemStore::<FileEntryImpl>::new_with_timeout_and_rename_fn(
            &FilesystemSpec {
                content_path: content_path.clone(),
                temp_path: temp_path.clone(),
                read_buffer_size: 1,
                ..Default::default()
            },
            |from, to| std::fs::rename(from, to),
        )
        .await?,
    );

    let keys = vec![digest.into()];
    let mut results = vec![None];
    drop(
        store
            .has_with_results(&keys, &mut results)
            .await
            .err_tip(|| "Failed to get_part"),
    );
    assert_eq!(results, vec![Some(0)]);

    check_storage_dir_empty(&content_path).await?;

    Ok(())
}

async fn wrap_update_zero_digest<F>(updater: F) -> Result<(), Error>
where
    F: AsyncFnOnce(DigestInfo, Arc<FilesystemStore>) -> Result<(), Error>,
{
    let digest = DigestInfo::new(Sha256::new().finalize().into(), 0);
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let store = FilesystemStore::<FileEntryImpl>::new_with_timeout_and_rename_fn(
        &FilesystemSpec {
            content_path: content_path.clone(),
            temp_path: temp_path.clone(),
            read_buffer_size: 1,
            ..Default::default()
        },
        |from, to| std::fs::rename(from, to),
    )
    .await?;
    updater(digest, store).await?;
    check_storage_dir_empty(&content_path).await?;
    check_storage_dir_empty(&temp_path).await?;
    Ok(())
}

#[nativelink_test]
async fn update_whole_file_with_zero_digest() -> Result<(), Error> {
    wrap_update_zero_digest(async |digest, store| {
        let temp_file_dir = make_temp_path("update_with_zero_digest");
        std::fs::create_dir_all(&temp_file_dir)?;
        let temp_file_path = Path::new(&temp_file_dir).join("zero-length-file");
        std::fs::write(&temp_file_path, b"")
            .err_tip(|| format!("Writing to {temp_file_path:?}"))?;
        let file_slot = fs::open_file(&temp_file_path, 0, 0).await?.into_inner();
        store
            .update_with_whole_file(
                digest,
                temp_file_path.into(),
                file_slot,
                UploadSizeInfo::ExactSize(0),
            )
            .await?;
        Ok(())
    })
    .await
}

#[nativelink_test]
async fn update_oneshot_with_zero_digest() -> Result<(), Error> {
    wrap_update_zero_digest(async |digest, store| store.update_oneshot(digest, Bytes::new()).await)
        .await
}

#[nativelink_test]
async fn update_with_zero_digest() -> Result<(), Error> {
    wrap_update_zero_digest(async |digest, store| {
        let (_writer, reader) = make_buf_channel_pair();
        store
            .update(digest, reader, UploadSizeInfo::ExactSize(0))
            .await
    })
    .await
}

#[nativelink_test]
async fn get_file_entry_for_zero_digest() -> Result<(), Error> {
    let digest = DigestInfo::new(Sha256::new().finalize().into(), 0);
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let store = FilesystemStore::<FileEntryImpl>::new_with_timeout_and_rename_fn(
        &FilesystemSpec {
            content_path: content_path.clone(),
            temp_path: temp_path.clone(),
            read_buffer_size: 1,
            ..Default::default()
        },
        |from, to| std::fs::rename(from, to),
    )
    .await?;

    let file_entry = store.get_file_entry_for_digest(&digest).await?;
    assert!(file_entry.is_empty());
    Ok(())
}

/// Regression test for: https://github.com/TraceMachina/nativelink/issues/495.
#[nativelink_test(flavor = "multi_thread")]
async fn update_file_future_drops_before_rename() -> Result<(), Error> {
    // Mutex can be used to signal to the rename function to pause execution.
    static RENAME_REQUEST_PAUSE_MUX: async_lock::Mutex<()> = async_lock::Mutex::new(());
    // Boolean used to know if the rename function is currently paused.
    static RENAME_IS_PAUSED: AtomicBool = AtomicBool::new(false);

    let digest = DigestInfo::try_new(HASH1, VALUE1.len())?;

    let content_path = make_temp_path("content_path");
    let store = Arc::pin(
        FilesystemStore::<FileEntryImpl>::new_with_timeout_and_rename_fn(
            &FilesystemSpec {
                content_path: content_path.clone(),
                temp_path: make_temp_path("temp_path"),
                eviction_policy: None,
                ..Default::default()
            },
            |from, to| {
                // If someone locked our mutex, it means we need to pause, so we
                // simply request a lock on the same mutex.
                if RENAME_REQUEST_PAUSE_MUX.try_lock().is_none() {
                    RENAME_IS_PAUSED.store(true, Ordering::Release);
                    while RENAME_REQUEST_PAUSE_MUX.try_lock().is_none() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    RENAME_IS_PAUSED.store(false, Ordering::Release);
                }
                std::fs::rename(from, to)
            },
        )
        .await?,
    );

    // Populate our first store entry.
    let first_file_entry = {
        store.update_oneshot(digest, VALUE1.into()).await?;
        store.get_file_entry_for_digest(&digest).await?
    };

    // 1. Request the next rename function to block.
    // 2. Request to replace our data.
    // 3. When we are certain that our rename function is paused, drop
    //    the replace/update future.
    // 4. Then drop the lock.
    {
        let rename_pause_request_lock = RENAME_REQUEST_PAUSE_MUX.lock().await;
        let mut update_fut = store.update_oneshot(digest, VALUE2.into()).boxed();

        loop {
            // Try to advance our update future.
            assert_eq!(poll!(&mut update_fut), Poll::Pending);

            // Once we are sure the rename function is paused break.
            if RENAME_IS_PAUSED.load(Ordering::Acquire) {
                break;
            }
            // Give a little time for background/kernel threads to run.
            sleep(Duration::from_millis(1)).await;
        }
        // Writing these out explicitly so users know this is what we are testing.
        // Note: The order they are dropped matters.
        drop(update_fut);
        drop(rename_pause_request_lock);
    }
    // Grab the newly inserted item in our store.
    let new_file_entry = store.get_file_entry_for_digest(&digest).await?;
    assert!(
        !Arc::ptr_eq(&first_file_entry, &new_file_entry),
        "Expected file entries to not be the same"
    );

    // Ensure the entry we inserted was properly flagged as moved (from temp -> content dir).
    new_file_entry
        .get_file_path_locked(move |file_path| async move {
            assert_eq!(
                file_path,
                OsString::from(format!("{content_path}/{DIGEST_FOLDER}/{digest}"))
            );
            Ok(())
        })
        .await?;

    Ok(())
}

#[nativelink_test]
async fn deleted_file_removed_from_store() -> Result<(), Error> {
    let digest = DigestInfo::try_new(HASH1, VALUE1.len())?;
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let store = Box::pin(
        FilesystemStore::<FileEntryImpl>::new_with_timeout_and_rename_fn(
            &FilesystemSpec {
                content_path: content_path.clone(),
                temp_path: temp_path.clone(),
                read_buffer_size: 1,
                ..Default::default()
            },
            |from, to| std::fs::rename(from, to),
        )
        .await?,
    );

    store.update_oneshot(digest, VALUE1.into()).await?;

    let stored_file_path = OsString::from(format!("{content_path}/{DIGEST_FOLDER}/{digest}"));
    std::fs::remove_file(stored_file_path)?;

    let get_part_res = store.get_part_unchunked(digest, 0, None).await;
    assert_eq!(get_part_res.unwrap_err().code, Code::NotFound);

    // Repeat with a string typed key.

    let string_key = StoreKey::new_str(STRING_NAME);

    store
        .update_oneshot(string_key.borrow(), VALUE2.into())
        .await
        .unwrap();

    let stored_file_path = OsString::from(format!("{content_path}/{STR_FOLDER}/{STRING_NAME}"));
    std::fs::remove_file(stored_file_path)?;

    let string_digest_get_part_res = store.get_part_unchunked(string_key, 0, None).await;
    assert_eq!(string_digest_get_part_res.unwrap_err().code, Code::NotFound);

    Ok(())
}

// Ensure that get_file_size() returns the correct number
// ceil(content length / block_size) * block_size
// assume block size 4K
// 1B data size = 4K size on disk
// 5K data size = 8K size on disk
#[nativelink_test]
async fn get_file_size_uses_block_size() -> Result<(), Error> {
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let value_1kb: String = "x".repeat(1024);
    let value_5kb: String = "xabcd".repeat(1024);

    let digest_1kb = DigestInfo::try_new(HASH1, value_1kb.len())?;
    let digest_5kb = DigestInfo::try_new(HASH2, value_5kb.len())?;

    let store = Box::pin(
        FilesystemStore::<FileEntryImpl>::new_with_timeout_and_rename_fn(
            &FilesystemSpec {
                content_path: content_path.clone(),
                temp_path: temp_path.clone(),
                read_buffer_size: 1,
                ..Default::default()
            },
            |from, to| std::fs::rename(from, to),
        )
        .await?,
    );

    store.update_oneshot(digest_1kb, value_1kb.into()).await?;
    let short_entry = store.get_file_entry_for_digest(&digest_1kb).await?;
    assert_eq!(short_entry.size_on_disk(), 4 * 1024);

    store.update_oneshot(digest_5kb, value_5kb.into()).await?;
    let long_entry = store.get_file_entry_for_digest(&digest_5kb).await?;
    assert_eq!(long_entry.size_on_disk(), 8 * 1024);
    Ok(())
}

#[nativelink_test]
async fn update_with_whole_file_closes_file() -> Result<(), Error> {
    #[expect(clippy::collection_is_never_read)] // TODO(jhpratt) investigate
    let mut permits = vec![];
    // Grab all permits to ensure only 1 permit is available.
    {
        wait_for_no_open_files().await?;
        while fs::OPEN_FILE_SEMAPHORE.available_permits() > 1 {
            permits.push(fs::get_permit().await);
        }
        assert_eq!(
            fs::OPEN_FILE_SEMAPHORE.available_permits(),
            1,
            "Expected 1 permit to be available"
        );
    }
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let value = "x".repeat(1024);

    let digest = DigestInfo::try_new(HASH1, value.len())?;

    let store = Box::pin(
        FilesystemStore::<FileEntryImpl>::new(&FilesystemSpec {
            content_path: content_path.clone(),
            temp_path: temp_path.clone(),
            read_buffer_size: 1,
            ..Default::default()
        })
        .await?,
    );
    store.update_oneshot(digest, value.clone().into()).await?;

    let file_path = OsString::from(format!("{temp_path}/dummy_file"));
    let mut file = fs::create_file(&file_path).await?;
    {
        file.write_all(value.as_bytes()).await?;
        file.as_mut().sync_all().await?;
        file.seek(tokio::io::SeekFrom::Start(0)).await?;
    }

    store
        .update_with_whole_file(
            digest,
            file_path,
            file,
            UploadSizeInfo::ExactSize(value.len() as u64),
        )
        .await?;
    Ok(())
}

// Ensure that update_with_whole_file() moves the file without making a copy.
#[cfg(target_family = "unix")]
#[nativelink_test]
async fn update_with_whole_file_uses_same_inode() -> Result<(), Error> {
    use std::os::unix::fs::MetadataExt;
    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let value: String = "x".repeat(1024);

    let digest = DigestInfo::try_new(HASH1, value.len())?;

    let store = Box::pin(
        FilesystemStore::<FileEntryImpl>::new_with_timeout_and_rename_fn(
            &FilesystemSpec {
                content_path: content_path.clone(),
                temp_path: temp_path.clone(),
                read_buffer_size: 1,
                ..Default::default()
            },
            |from, to| std::fs::rename(from, to),
        )
        .await?,
    );

    let file_path = OsString::from(format!("{temp_path}/dummy_file"));
    let original_inode = {
        let file = fs::create_file(&file_path).await?;
        let original_inode = file.as_ref().metadata().await?.ino();

        let result = store
            .update_with_whole_file(
                digest,
                file_path,
                file,
                UploadSizeInfo::ExactSize(value.len() as u64),
            )
            .await?;
        assert!(
            result.is_none(),
            "Expected filesystem store to consume the file"
        );
        original_inode
    };

    let expected_file_name = OsString::from(format!("{content_path}/{DIGEST_FOLDER}/{digest}"));
    let new_inode = fs::create_file(expected_file_name)
        .await
        .unwrap()
        .as_ref()
        .metadata()
        .await?
        .ino();
    assert_eq!(
        original_inode, new_inode,
        "Expected the same inode for the file"
    );

    Ok(())
}

#[nativelink_test]
async fn file_slot_taken_when_ready() -> Result<(), Error> {
    static FILE_SEMAPHORE: Semaphore = Semaphore::const_new(1);
    static WRITER_SEMAPHORE: Semaphore = Semaphore::const_new(1);
    static FILE_PERMIT: Mutex<Option<tokio::sync::SemaphorePermit<'_>>> = Mutex::new(None);
    static WRITER_PERMIT: Mutex<Option<tokio::sync::SemaphorePermit<'_>>> = Mutex::new(None);

    struct SingleSemaphoreHooks;
    impl FileEntryHooks for SingleSemaphoreHooks {
        async fn on_make_and_open(_encoded_file_path: &EncodedFilePath) -> Result<(), Error> {
            *FILE_PERMIT.lock() =
                Some(FILE_SEMAPHORE.acquire().await.map_err(|e| {
                    make_err!(Code::Internal, "Unable to acquire semaphore: {e:?}")
                })?);
            // Drop the writer permit now that we have one.
            WRITER_PERMIT.lock().take();
            Ok(())
        }
    }

    *WRITER_PERMIT.lock() = Some(WRITER_SEMAPHORE.acquire().await.unwrap());
    *FILE_PERMIT.lock() = Some(FILE_SEMAPHORE.acquire().await.unwrap());

    let content_path = make_temp_path("content_path");
    let temp_path = make_temp_path("temp_path");

    let value_1: String = "x".repeat(1024);
    let value_2: String = "y".repeat(1024);

    let digest_1 = DigestInfo::try_new(HASH1, value_1.len())?;
    let digest_2 = DigestInfo::try_new(HASH2, value_2.len())?;

    let store = Box::pin(
        FilesystemStore::<TestFileEntry<SingleSemaphoreHooks>>::new_with_timeout_and_rename_fn(
            &FilesystemSpec {
                content_path: content_path.clone(),
                temp_path: temp_path.clone(),
                read_buffer_size: 1,
                ..Default::default()
            },
            |from, to| std::fs::rename(from, to),
        )
        .await?,
    );

    let value_1 = Bytes::from(value_1);
    let value_2 = Bytes::from(value_2);

    let (mut writer_1, reader_1) = make_buf_channel_pair();
    let (mut writer_2, reader_2) = make_buf_channel_pair();
    let size_1 = UploadSizeInfo::ExactSize(value_1.len().try_into()?);
    let size_2 = UploadSizeInfo::ExactSize(value_2.len().try_into()?);
    let store_ref = &store;
    let update_1_fut = async move {
        let result = store_ref.update(digest_1, reader_1, size_1).await;
        FILE_PERMIT.lock().take();
        result
    };
    let update_2_fut = async move {
        let result = store_ref.update(digest_2, reader_2, size_2).await;
        FILE_PERMIT.lock().take();
        result
    };

    let writer_1_fut = async move {
        let _permit = WRITER_SEMAPHORE.acquire().await.unwrap();
        writer_1.send(value_1.slice(0..1)).await?;
        writer_1.send(value_1.slice(1..2)).await?;
        writer_1.send(value_1.slice(2..3)).await?;
        writer_1.send(value_1.slice(3..)).await?;
        writer_1.send_eof()?;
        Ok::<_, Error>(())
    };
    let writer_2_fut = async move {
        writer_2.send(value_2.slice(0..1)).await?;
        writer_2.send(value_2.slice(1..2)).await?;
        writer_2.send(value_2.slice(2..3)).await?;
        // Allow the update to get a file permit.
        FILE_PERMIT.lock().take();
        writer_2.send(value_2.slice(3..)).await?;
        writer_2.send_eof()?;
        Ok::<_, Error>(())
    };

    let (res_1, res_2, res_3, res_4) = tokio::time::timeout(Duration::from_secs(10), async move {
        tokio::join!(update_1_fut, update_2_fut, writer_1_fut, writer_2_fut)
    })
    .await
    .map_err(|_| make_err!(Code::Internal, "Deadlock detected"))?;
    res_1.merge(res_2).merge(res_3).merge(res_4)
}

// If we insert a file larger than the max_bytes eviction policy, it should be safely
// evicted, without deadlocking.
#[nativelink_test]
async fn safe_small_safe_eviction() -> Result<(), Error> {
    let store_spec = FilesystemSpec {
        content_path: "/tmp/nativelink/safe_fs".into(),
        temp_path: "/tmp/nativelink/safe_fs_temp".into(),
        eviction_policy: Some(EvictionPolicy {
            max_bytes: 1,
            ..Default::default()
        }),
        ..Default::default()
    };
    let store = Store::new(<FilesystemStore>::new(&store_spec).await?);

    // > than the max_bytes
    let bytes = 2;

    let data = make_random_data(bytes);
    let digest = DigestInfo::try_new(VALID_HASH, data.len()).unwrap();

    assert_eq!(
        store.has(digest).await,
        Ok(None),
        "Expected data to not exist in store"
    );

    store.update_oneshot(digest, data.clone().into()).await?;

    assert_eq!(
        store.has(digest).await,
        Ok(None),
        "Expected data to not exist in store, because eviction"
    );

    let (tx, mut rx) = make_buf_channel_pair();

    assert_eq!(
        store.get(digest, tx).await,
        Err(Error {
            code: Code::NotFound,
            messages: vec![format!(
                "{VALID_HASH}-{bytes} not found in filesystem store here"
            )],
        }),
        "Expected data to not exist in store, because eviction"
    );

    assert!(rx.recv().await.is_err());

    Ok(())
}
