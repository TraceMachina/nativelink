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

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use error::{make_err, Code, Error, ResultExt};
use native_link_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use native_link_util::common::DigestInfo;
use native_link_util::store_trait::{Store, UploadSizeInfo};

#[derive(Default)]
pub struct NoopStore;

impl NoopStore {
    pub fn new() -> Self {
        NoopStore {}
    }
}

#[async_trait]
impl Store for NoopStore {
    async fn has_with_results(
        self: Pin<&Self>,
        _digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        results.iter_mut().for_each(|r| *r = None);
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        _digest: DigestInfo,
        mut reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        // We need to drain the reader to avoid the writer complaining that we dropped
        // the connection prematurely.
        reader.drain().await.err_tip(|| "In NoopStore::update")?;
        Ok(())
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        _digest: DigestInfo,
        _writer: &mut DropCloserWriteHalf,
        _offset: usize,
        _length: Option<usize>,
    ) -> Result<(), Error> {
        Err(make_err!(Code::NotFound, "Not found in noop store"))
    }

    fn inner_store(self: Arc<Self>, _digest: Option<DigestInfo>) -> Arc<dyn Store> {
        self
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
