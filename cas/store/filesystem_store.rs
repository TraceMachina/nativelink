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
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use bytes::BytesMut;
use filetime::{set_file_atime, FileTime};
use futures::stream::{StreamExt, TryStreamExt};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom, Take};
use tokio::sync::RwLock;
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

struct DiskFile {
    content_path: String,
    delete_on_drop: bool,
    file_evicted_callback: Option<&'static (dyn Fn() + Sync)>,
}

struct FileEntry {
    digest: DigestInfo,
    file_size: u64,
    temp_path: Arc<String>,
    // The details of the file on disk are protected by an RwLock to ensure
    // that if the file is evicted from the content store it is placed into a
    // temporary directory until all references dropped.  This ensures the
    // immutibility of a FileEntry even if updates are performed.
    disk_file: RwLock<DiskFile>,
}

impl FileEntry {
    async fn read_file_part(&self, offset: u64, length: u64) -> Result<Take<fs::FileSlot<'_>>, Error> {
        let disk_file = self.disk_file.read().await;
        let mut file = fs::open_file(&disk_file.content_path)
            .await
            .err_tip(|| format!("Failed to open file in filesystem store {}", disk_file.content_path))?;

        file.seek(SeekFrom::Start(offset))
            .await
            .err_tip(|| format!("Failed to seek file: {}", disk_file.content_path))?;
        Ok(file.take(length))
    }
}

impl Debug for DiskFile {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("DiskFile")
            .field("content_path", &self.content_path)
            .field("delete_on_drop", &self.delete_on_drop)
            .finish()
    }
}

impl Debug for FileEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("FileEntry")
            .field("digest", &self.digest)
            .field("file_size", &self.file_size)
            .field("temp_path", &self.temp_path)
            .field("disk_file", &self.disk_file)
            .finish()
    }
}

fn temp_file_name() -> u64 {
    static NEXT_TEMP_FILENAME: AtomicU64 = AtomicU64::new(1);
    match NEXT_TEMP_FILENAME.fetch_add(1, Ordering::Relaxed) {
        // Edge case that the U64 wrapped around... 0 is a special flag to say
        // there is no temporary file, so grab the next number.  It's not going
        // to wrap around again that quickly, it's a U64!
        0 => NEXT_TEMP_FILENAME.fetch_add(1, Ordering::Relaxed),
        val => val,
    }
}

#[async_trait]
impl LenEntry for FileEntry {
    #[inline]
    fn len(&self) -> usize {
        self.file_size as usize
    }

    #[inline]
    async fn touch(&self) {
        let disk_file = self.disk_file.read().await;
        let content_path = disk_file.content_path.clone();
        let set_atime_fut = spawn_blocking(move || {
            set_file_atime(&content_path, FileTime::now())
                .err_tip(|| format!("Failed to touch file in filesystem store {}", content_path))
        });
        let res = match set_atime_fut.await {
            Ok(res) => res,
            Err(_) => Err(make_err!(
                Code::Internal,
                "Failed to change atime of file due to spawn failing"
            )),
        };
        if let Err(err) = res {
            log::error!("{:?}", err);
        }
    }

    #[inline]
    async fn unref(&self) {
        let mut disk_file = self.disk_file.write().await;
        if disk_file.delete_on_drop {
            // Already marked for deletion, don't bother doing it again.
            return;
        }

        let temp_file = to_full_path(&self.temp_path, &format!("{:x}", temp_file_name()));
        log::info!(
            "\x1b[0;31mFilesystem Store\x1b[0m: Deleting: {}",
            &disk_file.content_path
        );
        // It is possible (although extremely unlikely) that another thread is reading
        // this file while we want to delete it here. To prevent errors in either case
        // we rename the file (since that other thread would have an open file handle)
        // to the temp folder then delete it when the Arc reference is dropped.
        if let Err(err) = fs::rename(&disk_file.content_path, &temp_file).await {
            log::warn!(
                "Failed to rename file from {} to {} : {:?}",
                disk_file.content_path,
                temp_file,
                err
            );
        } else {
            disk_file.delete_on_drop = true;
            disk_file.content_path = temp_file;
        }
    }
}

impl Drop for DiskFile {
    fn drop(&mut self) {
        if !self.delete_on_drop {
            return;
        }
        let content_path = self.content_path.clone();
        let file_evicted_callback = self.file_evicted_callback.take();
        tokio::spawn(async move {
            log::info!("\x1b[0;31mFilesystem Store\x1b[0m: Store deleting: {}", &content_path);
            if let Err(err) = fs::remove_file(&content_path).await {
                log::warn!(
                    "\x1b[0;31mFilesystem Store\x1b[0m: Failed to remove file {} {:?}",
                    content_path,
                    err
                );
            }
            if let Some(callback) = file_evicted_callback {
                (callback)();
            }
        });
    }
}

#[inline]
fn to_full_path(folder: &str, name: &str) -> String {
    format!("{}/{}", folder, name)
}

#[inline]
fn to_full_path_from_digest(folder: &str, digest: &DigestInfo) -> String {
    format!("{}/{}-{}", folder, digest.str(), digest.size_bytes)
}

async fn add_files_to_cache(
    evicting_map: &EvictingMap<Arc<FileEntry>, SystemTime>,
    anchor_time: &SystemTime,
    temp_path: &Arc<String>,
    content_path: &Arc<String>,
) -> Result<(), Error> {
    fn make_digest(file_name: &str) -> Result<DigestInfo, Error> {
        let (hash, size) = file_name.split_once('-').err_tip(|| "")?;
        let size = i64::from_str_radix(size, 10)?;
        DigestInfo::try_new(hash, size)
    }

    async fn process_entry(
        evicting_map: &EvictingMap<Arc<FileEntry>, SystemTime>,
        file_name: &str,
        atime: SystemTime,
        file_size: u64,
        anchor_time: &SystemTime,
        temp_path: &Arc<String>,
        content_path: &Arc<String>,
    ) -> Result<(), Error> {
        let digest = make_digest(&file_name)?;

        let file_entry = FileEntry {
            digest: digest.clone(),
            file_size,
            temp_path: temp_path.clone(),
            disk_file: RwLock::new(DiskFile {
                content_path: to_full_path_from_digest(&content_path, &digest),
                delete_on_drop: false,
                file_evicted_callback: None,
            }),
        };
        let time_since_anchor = anchor_time
            .duration_since(atime)
            .map_err(|_| make_input_err!("File access time newer than now"))?;
        evicting_map
            .insert_with_time(digest, Arc::new(file_entry), time_since_anchor.as_secs() as i32)
            .await;
        Ok(())
    }

    let mut file_infos: Vec<(String, SystemTime, u64)> = {
        let (_permit, dir_handle) = fs::read_dir(format!("{}/", content_path))
            .await
            .err_tip(|| "Failed opening content directory for iterating in filesystem store")?
            .into_inner();

        let read_dir_stream = ReadDirStream::new(dir_handle);
        read_dir_stream
            .then(|dir_entry| async move {
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
            &temp_path,
            &content_path,
        )
        .await;
        if let Err(err) = result {
            log::warn!(
                "Could not add file to eviction cache, so deleting: {} - {:?}",
                file_name,
                err
            );
            // Ignore result.
            let _ = fs::remove_file(format!("{}/{}", &content_path, &file_name)).await;
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

pub struct FilesystemStore {
    temp_path: Arc<String>,
    content_path: Arc<String>,
    evicting_map: EvictingMap<Arc<FileEntry>, SystemTime>,
    read_buffer_size: usize,
    file_evicted_callback: Option<&'static (dyn Fn() + Sync)>,
}

impl FilesystemStore {
    pub async fn new_with_callback(
        config: &config::backends::FilesystemStore,
        file_evicted_callback: &'static (dyn Fn() + Sync),
    ) -> Result<Self, Error> {
        let mut me = Self::new(config).await?;
        me.file_evicted_callback = Some(file_evicted_callback);
        Ok(me)
    }

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

        let temp_path = Arc::new(config.temp_path.clone());
        let content_path = Arc::new(config.content_path.clone());
        add_files_to_cache(&evicting_map, &now, &temp_path, &content_path).await?;
        prune_temp_path(&temp_path.as_ref()).await?;

        let read_buffer_size = if config.read_buffer_size == 0 {
            DEFAULT_BUFF_SIZE
        } else {
            config.read_buffer_size as usize
        };
        let store = Self {
            temp_path,
            content_path,
            evicting_map,
            read_buffer_size,
            file_evicted_callback: None,
        };
        Ok(store)
    }

    async fn get_entry(&self, digest: &DigestInfo) -> Result<Arc<FileEntry>, Error> {
        self.evicting_map
            .get(&digest)
            .await
            .ok_or_else(|| make_err!(Code::NotFound, "not found in filesystem store"))
    }

    pub async fn hard_link(&self, digest: &DigestInfo, dst: impl AsRef<Path>) -> Result<(), Error> {
        let entry = self.get_entry(digest).await?;
        let disk_file = entry.disk_file.read().await;
        fs::hard_link(&disk_file.content_path, dst).await
    }

    pub fn get_file_for_digest(&self, digest: &DigestInfo) -> String {
        to_full_path_from_digest(self.content_path.as_ref(), &digest)
    }

    async fn update_file<'a>(
        self: Pin<&Self>,
        temp_loc: &str,
        mut temp_file: fs::FileSlot<'a>,
        digest: DigestInfo,
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
                .err_tip(|| format!("Failed to write data into filesystem store {}", temp_loc))?;
            file_size += data_len as u64;
        }

        temp_file
            .as_ref()
            .sync_all()
            .await
            .err_tip(|| format!("Failed to sync_data in filesystem store {}", temp_loc))?;

        drop(temp_file);

        let entry = Arc::new(FileEntry {
            digest: digest.clone(),
            file_size,
            temp_path: self.temp_path.clone(),
            disk_file: RwLock::new(DiskFile {
                content_path: to_full_path_from_digest(&self.content_path, &digest),
                delete_on_drop: false,
                file_evicted_callback: self.file_evicted_callback,
            }),
        });

        // In order to do an immutable swap in the map here, we take the write
        // lock for the new entry, then we swap out the entry in the map and
        // then take the write lock of any existing entry, then we swap the
        // files on disk to ensure it's all in sequence.
        let new_disk_file = entry.disk_file.write().await;
        if let Some(old_item) = self.evicting_map.insert(digest, entry.clone()).await {
            // Move the old file out of the way of the new file
            old_item.unref().await;
        }
        // Now the content_path is free, so we can take it
        fs::rename(&temp_loc, &new_disk_file.content_path).await?;

        Ok(())
    }
}

#[async_trait]
impl StoreTrait for FilesystemStore {
    async fn has(self: Pin<&Self>, digest: DigestInfo) -> Result<Option<usize>, Error> {
        Ok(self.evicting_map.size_for_key(&digest).await)
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let temp_full_path = to_full_path(&self.temp_path, &format!("{:x}", temp_file_name()));

        let temp_file = fs::create_file(&temp_full_path)
            .await
            .err_tip(|| "Failed to create temp file in filesystem store")?;

        if let Err(err) = self.update_file(&temp_full_path, temp_file, digest, reader).await {
            let result = fs::remove_file(temp_full_path)
                .await
                .err_tip(|| "Failed to delete temp file in filesystem store");
            if result.is_err() {
                return Result::<(), Error>::Err(err).merge(result);
            }
            return Err(err);
        }

        Ok(())
    }

    async fn get_part(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut writer: DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let entry = self.get_entry(&digest).await?;
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
