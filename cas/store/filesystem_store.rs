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

use std::fmt::{Debug, Formatter};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use async_lock::RwLock;
use async_trait::async_trait;
use bytes::BytesMut;
use filetime::{set_file_atime, FileTime};
use futures::stream::{StreamExt, TryStreamExt};
use futures::Future;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom, Take};
use tokio::task::spawn_blocking;
use tokio_stream::wrappers::ReadDirStream;

use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::{fs, log, DigestInfo};
use config;
use error::{make_err, make_input_err, Code, Error, ResultExt};
use evicting_map::{EvictingMap, LenEntry};
use traits::{StoreTrait, UploadSizeInfo};

// Default size to allocate memory of the buffer when reading files.
const DEFAULT_BUFF_SIZE: usize = 32 * 1024;

#[derive(Debug)]
pub struct SharedContext {
    // Used in testing to know how many active drop() spawns are running.
    // TODO(allada) It is probably a good idea to use a spin lock during
    // destruction of the store to ensure that all files are actually
    // deleted (similar to how it is done in tests).
    pub active_drop_spawns: AtomicU64,
    temp_path: String,
    content_path: String,
}

#[derive(Eq, PartialEq, Debug)]
enum PathType {
    Content,
    Temp,
}

// Note: We don't store the full path of the file because it would cause
// a lot of needless memeory bloat. There's a high chance we'll end up with a
// lot of small files, so to prevent storing duplicate data, we store an Arc
// to the path of the directory where the file is stored and the packed digest.
// Resulting in usize + sizeof(DigestInfo).
type FileNameDigest = DigestInfo;
pub struct EncodedFilePath {
    shared_context: Arc<SharedContext>,
    path_type: PathType,
    digest: FileNameDigest,
}

impl EncodedFilePath {
    #[inline]
    fn get_file_path(&self) -> String {
        get_file_path_raw(&self.path_type, self.shared_context.as_ref(), &self.digest)
    }
}

#[inline]
fn get_file_path_raw(path_type: &PathType, shared_context: &SharedContext, digest: &DigestInfo) -> String {
    let folder = match path_type {
        PathType::Content => &shared_context.content_path,
        PathType::Temp => &shared_context.temp_path,
    };
    to_full_path_from_digest(folder, &digest)
}

impl Drop for EncodedFilePath {
    fn drop(&mut self) {
        // `drop()` can be called during shutdown, so we use `path_type` flag to know if the
        // file actually needs to be deleted.
        if self.path_type == PathType::Content {
            return;
        }

        let file_path = self.get_file_path();
        let shared_context = self.shared_context.clone();
        shared_context.active_drop_spawns.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(async move {
            log::info!("\x1b[0;31mFilesystem Store\x1b[0m: Deleting: {}", &file_path);
            let result = fs::remove_file(&file_path)
                .await
                .err_tip(|| format!("Failed to remove file {}", file_path));
            if let Err(err) = result {
                log::warn!("\x1b[0;31mFilesystem Store\x1b[0m: {:?}", err);
            }
            shared_context.active_drop_spawns.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

#[inline]
fn to_full_path_from_digest(folder: &str, digest: &DigestInfo) -> String {
    format!("{}/{}-{}", folder, digest.str(), digest.size_bytes)
}

#[async_trait]
pub trait FileEntry: LenEntry + Send + Sync + Debug + 'static {
    /// Responsible for creating the underlying FileEntry.
    fn create(file_size: u64, encoded_file_path: RwLock<EncodedFilePath>) -> Self;

    /// Creates a (usually) temp file, opens it and returns the path to the temp file.
    async fn make_and_open_file(
        encoded_file_path: EncodedFilePath,
    ) -> Result<(Self, fs::FileSlot<'static>, String), Error>
    where
        Self: Sized;

    /// Returns the underlying reference to where the filesize is stored.
    fn get_file_size<'a>(&'a mut self) -> &'a mut u64;

    /// Gets the underlying EncodedfilePath.
    fn get_encoded_file_path<'a>(&'a self) -> &'a RwLock<EncodedFilePath>;

    /// Returns a reader that will read part of the underlying file.
    async fn read_file_part(&self, offset: u64, length: u64) -> Result<Take<fs::FileSlot<'_>>, Error>;

    /// This function is a safe way to extract the file name of the underlying file. To protect users from
    /// accidentally creating undefined behavior we encourage users to do the logic they need to do with
    /// the filename inside this function instead of extracting the filename and doing the logic outside.
    /// This is because the filename is not guaranteed to exist after this function returns, however inside
    /// the callback the file is always guaranteed to exist and immutable.
    /// DO NOT USE THIS FUNCTION TO EXTRACT THE FILENAME AND STORE IT FOR LATER USE.
    async fn get_file_path_locked<T, Fut: Future<Output = Result<T, Error>> + Send, F: FnOnce(String) -> Fut + Send>(
        &self,
        handler: F,
    ) -> Result<T, Error>;
}

pub struct FileEntryImpl {
    file_size: u64,
    encoded_file_path: RwLock<EncodedFilePath>,
}

impl FileEntryImpl {
    pub fn get_shared_context_for_test(&mut self) -> Arc<SharedContext> {
        self.encoded_file_path.get_mut().shared_context.clone()
    }
}

#[async_trait]
impl FileEntry for FileEntryImpl {
    fn create(file_size: u64, encoded_file_path: RwLock<EncodedFilePath>) -> Self {
        Self {
            file_size,
            encoded_file_path,
        }
    }

    /// This encapsolates the logic for the edge case of if the file fails to create
    /// the cleanup of the file is handled without creating a FileEntry, which would
    /// try to cleanup the file as well during drop().
    async fn make_and_open_file(
        encoded_file_path: EncodedFilePath,
    ) -> Result<(FileEntryImpl, fs::FileSlot<'static>, String), Error> {
        let temp_full_path = encoded_file_path.get_file_path();
        let temp_file_result = fs::create_file(&temp_full_path)
            .await
            .err_tip(|| format!("Failed to create {} in filesystem store", temp_full_path));

        match temp_file_result {
            Ok(file) => {
                Ok((
                    <FileEntryImpl as FileEntry>::create(
                        0, /* Unknown yet, we will fill it in later */
                        RwLock::new(encoded_file_path),
                    ),
                    file,
                    temp_full_path,
                ))
            }
            Err(mut err) => {
                let remove_result = fs::remove_file(&temp_full_path)
                    .await
                    .err_tip(|| format!("Failed to remove file {} in filesystem store", temp_full_path));
                if let Err(remove_err) = remove_result {
                    err = err.merge(remove_err);
                }
                log::warn!("\x1b[0;31mFilesystem Store\x1b[0m: {:?}", err);
                Err(err)
            }
        }
    }

    fn get_file_size<'a>(&'a mut self) -> &'a mut u64 {
        &mut self.file_size
    }

    fn get_encoded_file_path<'a>(&'a self) -> &'a RwLock<EncodedFilePath> {
        &self.encoded_file_path
    }

    async fn read_file_part(&self, offset: u64, length: u64) -> Result<Take<fs::FileSlot<'_>>, Error> {
        let (mut file, full_content_path_for_debug_only) = self
            .get_file_path_locked(|full_content_path| async move {
                let file = fs::open_file(&full_content_path)
                    .await
                    .err_tip(|| format!("Failed to open file in filesystem store {}", full_content_path))?;
                Ok((file, full_content_path))
            })
            .await?;

        file.seek(SeekFrom::Start(offset))
            .await
            .err_tip(|| format!("Failed to seek file: {}", full_content_path_for_debug_only))?;
        Ok(file.take(length))
    }

    async fn get_file_path_locked<T, Fut: Future<Output = Result<T, Error>> + Send, F: FnOnce(String) -> Fut + Send>(
        &self,
        handler: F,
    ) -> Result<T, Error> {
        let encoded_file_path = self.get_encoded_file_path().read().await;
        handler(encoded_file_path.get_file_path()).await
    }
}

impl Debug for FileEntryImpl {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("FileEntryImpl")
            .field("file_size", &self.file_size)
            .field("encoded_file_path", &"<behind mutex>")
            .finish()
    }
}

fn make_temp_digest(digest: &mut DigestInfo) {
    static DELETE_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
    digest.packed_hash[24..].clone_from_slice(&DELETE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
}

#[async_trait]
impl LenEntry for FileEntryImpl {
    #[inline]
    fn len(&self) -> usize {
        self.file_size as usize
    }

    #[inline]
    async fn touch(&self) {
        let result = self
            .get_file_path_locked(move |full_content_path| async move {
                Ok(spawn_blocking(move || {
                    set_file_atime(&full_content_path, FileTime::now())
                        .err_tip(|| format!("Failed to touch file in filesystem store {}", full_content_path))
                })
                .await
                .map_err(|e| {
                    make_err!(
                        Code::Internal,
                        "Failed to change atime of file due to spawn failing {:?}",
                        e
                    )
                })??)
            })
            .await;
        if let Err(e) = result {
            log::error!("{}", e);
        }
    }

    // unref() only triggers when an item is removed from the eviction_map. It is possible
    // that another place in code has a reference to `FileEntryImpl` and may later read the
    // file. To support this edge case, we first move the file to a temp file and point
    // target file location to the new temp file. `unref()` should only ever be called once.
    #[inline]
    async fn unref(&self) {
        {
            let mut encoded_file_path = self.encoded_file_path.write().await;
            if encoded_file_path.path_type == PathType::Temp {
                // We are already a temp file that is now marked for deletion on drop.
                // This is very rare, but most likely the rename into the content path failed.
                return;
            }
            let from_path = encoded_file_path.get_file_path();
            let mut new_digest = encoded_file_path.digest.clone();
            make_temp_digest(&mut new_digest);

            let to_path = to_full_path_from_digest(&encoded_file_path.shared_context.temp_path, &new_digest);

            log::info!(
                "\x1b[0;31mFilesystem Store\x1b[0m: Unref {}, moving file {} to {}",
                encoded_file_path.digest.str(),
                &from_path,
                &to_path
            );
            if let Err(err) = fs::rename(&from_path, &to_path).await {
                log::warn!("Failed to rename file from {} to {} : {:?}", from_path, to_path, err);
            } else {
                encoded_file_path.path_type = PathType::Temp;
                encoded_file_path.digest = new_digest;
            }
        }
    }
}

#[inline]
pub fn digest_from_filename(file_name: &str) -> Result<DigestInfo, Error> {
    let (hash, size) = file_name.split_once('-').err_tip(|| "")?;
    let size = i64::from_str_radix(size, 10)?;
    DigestInfo::try_new(hash, size)
}

async fn add_files_to_cache<Fe: FileEntry>(
    evicting_map: &EvictingMap<Arc<Fe>, SystemTime>,
    anchor_time: &SystemTime,
    shared_context: &Arc<SharedContext>,
) -> Result<(), Error> {
    async fn process_entry<Fe: FileEntry>(
        evicting_map: &EvictingMap<Arc<Fe>, SystemTime>,
        file_name: &str,
        atime: SystemTime,
        file_size: u64,
        anchor_time: &SystemTime,
        shared_context: &Arc<SharedContext>,
    ) -> Result<(), Error> {
        let digest = digest_from_filename(&file_name)?;

        let file_entry = Fe::create(
            file_size,
            RwLock::new(EncodedFilePath {
                shared_context: shared_context.clone(),
                path_type: PathType::Content,
                digest: digest.clone(),
            }),
        );
        let time_since_anchor = anchor_time
            .duration_since(atime)
            .map_err(|_| make_input_err!("File access time newer than now"))?;
        evicting_map
            .insert_with_time(digest, Arc::new(file_entry), time_since_anchor.as_secs() as i32)
            .await;
        Ok(())
    }

    let mut file_infos: Vec<(String, SystemTime, u64)> = {
        let (_permit, dir_handle) = fs::read_dir(format!("{}/", shared_context.content_path))
            .await
            .err_tip(|| "Failed opening content directory for iterating in filesystem store")?
            .into_inner();

        let read_dir_stream = ReadDirStream::new(dir_handle);
        read_dir_stream
            .map(|dir_entry| async move {
                let dir_entry = dir_entry.unwrap();
                let file_name = dir_entry.file_name().into_string().unwrap();
                let metadata = dir_entry
                    .metadata()
                    .await
                    .err_tip(|| "Failed to get metadata in filesystem store")?;
                let atime = match metadata.accessed() {
                    Ok(atime) => atime,
                    Err(err) => {
                        panic!(
                            "{}{}{} : {} {:?}",
                            "It appears this filesystem does not support access time. ",
                            "Please configure this program to run on a drive that supports ",
                            "atime",
                            file_name,
                            err
                        );
                    }
                };
                Result::<(String, SystemTime, u64), Error>::Ok((file_name, atime, metadata.len()))
            })
            .buffer_unordered(200)
            .try_collect()
            .await?
    };

    file_infos.sort_by(|a, b| a.1.cmp(&b.1));
    for (file_name, atime, file_size) in file_infos {
        let result = process_entry(
            &evicting_map,
            &file_name,
            atime,
            file_size,
            &anchor_time,
            &shared_context,
        )
        .await;
        if let Err(err) = result {
            log::warn!(
                "Could not add file to eviction cache, so deleting: {} - {:?}",
                file_name,
                err
            );
            // Ignore result.
            let _ = fs::remove_file(format!("{}/{}", &shared_context.content_path, &file_name)).await;
        }
    }
    Ok(())
}

async fn prune_temp_path(temp_path: &str) -> Result<(), Error> {
    let (_permit, dir_handle) = fs::read_dir(temp_path)
        .await
        .err_tip(|| "Failed opening temp directory to prune partial downloads in filesystem store")?
        .into_inner();

    let mut read_dir_stream = ReadDirStream::new(dir_handle);
    while let Some(dir_entry) = read_dir_stream.next().await {
        let path = dir_entry?.path();
        if let Err(err) = fs::remove_file(&path).await {
            log::warn!("Failed to delete file in filesystem store {:?} : {:?}", &path, err);
        }
    }
    Ok(())
}

pub struct FilesystemStore<Fe: FileEntry = FileEntryImpl> {
    shared_context: Arc<SharedContext>,
    evicting_map: EvictingMap<Arc<Fe>, SystemTime>,
    read_buffer_size: usize,
}

impl<Fe: FileEntry> FilesystemStore<Fe> {
    pub async fn new(config: &config::backends::FilesystemStore) -> Result<Self, Error> {
        let now = SystemTime::now();

        let empty_policy = config::backends::EvictionPolicy::default();
        let eviction_policy = config.eviction_policy.as_ref().unwrap_or(&empty_policy);
        let evicting_map = EvictingMap::new(eviction_policy, now);

        fs::create_dir_all(&config.temp_path)
            .await
            .err_tip(|| format!("Failed to temp directory {:?}", &config.temp_path))?;
        fs::create_dir_all(&config.content_path)
            .await
            .err_tip(|| format!("Failed to content directory {:?}", &config.content_path))?;

        let shared_context = Arc::new(SharedContext {
            active_drop_spawns: AtomicU64::new(0),
            temp_path: config.temp_path.clone(),
            content_path: config.content_path.clone(),
        });
        add_files_to_cache(&evicting_map, &now, &shared_context).await?;
        prune_temp_path(&shared_context.temp_path).await?;

        let read_buffer_size = if config.read_buffer_size == 0 {
            DEFAULT_BUFF_SIZE
        } else {
            config.read_buffer_size as usize
        };
        let store = Self {
            shared_context,
            evicting_map,
            read_buffer_size,
        };
        Ok(store)
    }

    pub async fn get_file_entry_for_digest(&self, digest: &DigestInfo) -> Result<Arc<Fe>, Error> {
        self.evicting_map
            .get(&digest)
            .await
            .ok_or_else(|| make_err!(Code::NotFound, "not found in filesystem store"))
    }

    async fn update_file<'a>(
        self: Pin<&Self>,
        mut entry: Fe,
        mut temp_file: fs::FileSlot<'a>,
        final_digest: DigestInfo,
        mut reader: DropCloserReadHalf,
    ) -> Result<(), Error> {
        let mut file_size = 0;
        loop {
            let mut data = reader
                .recv()
                .await
                .err_tip(|| "Failed to receive data in filesystem store")?;
            let data_len = data.len();
            if data_len == 0 {
                break; // EOF.
            }
            temp_file
                .write_all_buf(&mut data)
                .await
                .err_tip(|| "Failed to write data into filesystem store")?;
            file_size += data_len as u64;
        }

        temp_file
            .as_ref()
            .sync_all()
            .await
            .err_tip(|| "Failed to sync_data in filesystem store")?;

        drop(temp_file);

        *entry.get_file_size() = file_size;
        let entry = Arc::new(entry);

        // This sequence of events is quite ticky to understand due to the amount of triggers that
        // happen, async'ness of it and the locking. So here is a breakdown of what happens:
        // 1. Here will hold a write lock on any file operations of this FileEntry.
        // 2. Then insert the entry into the evicting map. This may trigger an eviction of other
        //    entries.
        // 3. Eviction triggers `unref()`, which grabs a write lock on the evicted FileEntrys
        //    during the rename.
        // 4. It should be impossible for items to be added while eviction is happening, so there
        //    should not be a deadlock possability. However, it is possible for the new FileEntry
        //    to be evicted before the file is moved into place. Eviction of the newly inserted
        //    item is not possible within the `insert()` call because the write lock inside the
        //    eviction map. If an eviction of new item happens after `insert()` but before
        //    `rename()` then we get to finish our operation because the `unref()` of the new item
        //    will be blocked on us because we currently have the lock.
        // 5. Move the file into place. Since we hold a write lock still anyone that gets our new
        //    FileEntry (which has not yet been placed on disk) will not be able to read the file's
        //    contents until we relese the lock.
        {
            let mut encoded_file_path = entry.get_encoded_file_path().write().await;
            let final_path = get_file_path_raw(
                &PathType::Content,
                encoded_file_path.shared_context.as_ref(),
                &final_digest,
            );

            self.evicting_map.insert(final_digest.clone(), entry.clone()).await;

            let result = fs::rename(encoded_file_path.get_file_path(), &final_path)
                .await
                .err_tip(|| format!("Failed to rename temp file to final path {}", final_path));

            // In the event our move from temp file to final file fails we need to ensure we remove
            // the entry from our map.
            // Remember: At this point it is possible for another thread to have a reference to
            // `entry`, so we can't delete the file, only drop() should ever delete files.
            if let Err(err) = result {
                log::warn!("{}", err);
                // Warning: To prevent deadlock we need to release our lock or during `remove_if()`
                // it will call `unref()`, which triggers a write-lock on `encoded_file_path`.
                drop(encoded_file_path);
                // It is possible that the item in our map is no longer the item we inserted,
                // So, we need to conditionally remove it only if the pointers are the same.
                self.evicting_map
                    .remove_if(&final_digest, |map_entry| Arc::<Fe>::ptr_eq(map_entry, &entry))
                    .await;
                return Err(err);
            }
            encoded_file_path.path_type = PathType::Content;
            encoded_file_path.digest = final_digest;
            Ok(())
        }
    }
}

#[async_trait]
impl<Fe: FileEntry> StoreTrait for FilesystemStore<Fe> {
    async fn has(self: Pin<&Self>, digest: DigestInfo) -> Result<Option<usize>, Error> {
        Ok(self.evicting_map.size_for_key(&digest).await)
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let mut temp_digest = digest.clone();
        make_temp_digest(&mut temp_digest);

        let (entry, temp_file, temp_full_path) = Fe::make_and_open_file(EncodedFilePath {
            shared_context: self.shared_context.clone(),
            path_type: PathType::Temp,
            digest: temp_digest,
        })
        .await?;

        self.update_file(entry, temp_file, digest, reader)
            .await
            .err_tip(|| format!("While processing with temp file {}", temp_full_path))
    }

    async fn get_part(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut writer: DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let entry = self
            .evicting_map
            .get(&digest)
            .await
            .ok_or_else(|| make_err!(Code::NotFound, "not found in filesystem store"))?;
        let mut file = entry
            .read_file_part(offset as u64, length.unwrap_or(usize::MAX) as u64)
            .await?;

        let mut buf = BytesMut::with_capacity(length.unwrap_or(self.read_buffer_size));
        loop {
            file.read_buf(&mut buf)
                .await
                .err_tip(|| "Failed to read data in filesystem store")?;
            if buf.len() == 0 {
                break; // EOF.
            }
            writer
                .send(buf.split().freeze())
                .await
                .err_tip(|| "Failed to send chunk in filesystem store get_part")?;
        }
        writer
            .send_eof()
            .await
            .err_tip(|| "Filed to send EOF in filesystem store get_part")?;

        Ok(())
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
