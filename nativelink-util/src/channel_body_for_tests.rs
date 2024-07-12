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

use std::pin::Pin;
use std::task::{ready, Context};

use bytes::Bytes;
use futures::task::Poll;
use hyper::body::{Body, Frame};
use pin_project_lite::pin_project;
use tokio::sync::mpsc;

const DEFAULT_CHANNEL_BODY_BUFFER_SIZE: usize = 32;

pin_project! {
    pub struct ChannelBody {
        #[pin]
        rx: mpsc::Receiver<Frame<Bytes>>,
    }
}

// Note: At the moment this is only used in a few tests after tonic removed its
// concrete Body type. See `nativelink_util::buf_channel` for channel
// implementations used in non-test code and see the tests for example usage.
impl ChannelBody {
    #[must_use]
    pub fn new() -> (mpsc::Sender<Frame<Bytes>>, Self) {
        Self::with_buffer_size(DEFAULT_CHANNEL_BODY_BUFFER_SIZE)
    }

    #[must_use]
    pub fn with_buffer_size(buffer_size: usize) -> (mpsc::Sender<Frame<Bytes>>, Self) {
        let (tx, rx) = mpsc::channel(buffer_size);
        (tx, Self { rx })
    }
}

impl Body for ChannelBody {
    type Data = Bytes;
    type Error = tonic::Status;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let frame = ready!(self.project().rx.poll_recv(cx));
        Poll::Ready(frame.map(Ok))
    }

    fn is_end_stream(&self) -> bool {
        self.rx.is_closed()
    }
}
