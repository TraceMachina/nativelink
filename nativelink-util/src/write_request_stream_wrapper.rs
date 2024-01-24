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
use std::task::{Context, Poll};

use futures::{Stream, StreamExt};
use nativelink_error::{error_if, Error, ResultExt};
use nativelink_proto::google::bytestream::WriteRequest;

use crate::resource_info::ResourceInfo;

#[derive(Debug)]
pub struct WriteRequestStreamWrapper<T, E>
where
    E: Into<Error>,
    T: Stream<Item = Result<WriteRequest, E>> + Unpin,
{
    pub instance_name: String,
    pub uuid: Option<String>,
    pub hash: String,
    pub expected_size: usize,
    pub bytes_received: usize,
    stream: T,
    first_msg: Option<WriteRequest>,
    first_sent: bool,
    write_finished: bool,
}

impl<T, E> WriteRequestStreamWrapper<T, E>
where
    E: Into<Error>,
    T: Stream<Item = Result<WriteRequest, E>> + Unpin,
{
    pub async fn from(mut stream: T) -> Result<WriteRequestStreamWrapper<T, E>, Error> {
        let first_msg = stream
            .next()
            .await
            .err_tip(|| "Error receiving first message in stream")?
            .err_tip(|| "Expected WriteRequest struct in stream")?;

        let resource_info = ResourceInfo::new(&first_msg.resource_name, true).err_tip(|| {
            format!(
                "Could not extract resource info from first message of stream: {}",
                first_msg.resource_name
            )
        })?;
        let instance_name = resource_info.instance_name.to_string();
        let hash = resource_info.hash.to_string();
        let expected_size = resource_info.expected_size;
        let uuid = resource_info.uuid.map(|v| v.to_string());
        let write_finished = first_msg.finish_write;

        Ok(WriteRequestStreamWrapper {
            instance_name,
            uuid,
            hash,
            expected_size,
            bytes_received: 0,
            stream,
            first_msg: Some(first_msg),
            first_sent: false,
            write_finished,
        })
    }

    pub fn reset_for_retry(&mut self) -> bool {
        if self.first_msg.is_some() {
            self.bytes_received = 0;
            self.first_sent = false;
            true
        } else {
            false
        }
    }
}

impl<T, E> Stream for WriteRequestStreamWrapper<T, E>
where
    E: Into<Error>,
    T: Stream<Item = Result<WriteRequest, E>> + Unpin,
{
    type Item = Result<WriteRequest, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<WriteRequest, Error>>> {
        if !self.first_sent {
            if let Some(first_msg) = self.first_msg.clone() {
                self.bytes_received += first_msg.data.len();
                self.first_sent = true;
                return Poll::Ready(Some(Ok(first_msg)));
            }
        }
        if self.write_finished {
            error_if!(
                self.bytes_received != self.expected_size,
                "Did not send enough data. Expected {}, but so far received {}",
                self.expected_size,
                self.bytes_received
            );
            return Poll::Ready(None); // Previous message said it was the last msg.
        }
        error_if!(
            self.bytes_received > self.expected_size,
            "Sent too much data. Expected {}, but so far received {}",
            self.expected_size,
            self.bytes_received
        );
        match Pin::new(&mut self.stream).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(maybe_message)) => {
                if let Ok(message) = &maybe_message {
                    // Upon a successful second message, the first needs to be
                    // discarded as we can't use it to retry any more.
                    self.first_msg.take();
                    self.write_finished = message.finish_write;
                    self.bytes_received += message.data.len();
                }
                Poll::Ready(Some(
                    maybe_message.err_tip(|| format!("Stream error at byte {}", self.bytes_received)),
                ))
            }
        }
    }
}
