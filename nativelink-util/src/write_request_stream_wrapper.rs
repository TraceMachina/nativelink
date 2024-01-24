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
use nativelink_error::{make_input_err, Error, ResultExt};
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
    second_msg: Option<WriteRequest>,
    message_count: usize,
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

        Ok(WriteRequestStreamWrapper {
            instance_name,
            uuid,
            hash,
            expected_size,
            bytes_received: 0,
            stream,
            first_msg: Some(first_msg),
            // Since Tonic reads the second message before determining a failure
            // with the first message, we need to keep both in memory until the
            // third message.  This is unfortunate, but should not be too
            // expensive as each write request is fairly small and the backing
            // Bytes is shared between the cloned instances.
            second_msg: None,
            message_count: 0,
            write_finished: false,
        })
    }

    pub fn reset_for_retry(&mut self) -> bool {
        if self.first_msg.is_some() {
            self.bytes_received = 0;
            self.write_finished = false;
            self.message_count = 0;
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
        // If the stream said that the previous message was the last one, then
        // return a stream EOF (i.e. None).
        if self.write_finished {
            if self.bytes_received != self.expected_size {
                return Poll::Ready(Some(Err(make_input_err!(
                    "Did not send enough data. Expected {}, but so far received {}",
                    self.expected_size,
                    self.bytes_received
                ))));
            }
            return Poll::Ready(None);
        }

        // Gets the next message, this is either the cached first or second
        // message or a subsequent message from the wrapped Stream.
        let maybe_message = if self.message_count == 0 {
            if let Some(first_msg) = self.first_msg.clone() {
                Ok(first_msg)
            } else {
                Err(make_input_err!("First message was lost in write stream wrapper"))
            }
        } else if self.message_count == 1 && self.second_msg.is_some() {
            Ok(self.second_msg.clone().unwrap())
        } else {
            match Pin::new(&mut self.stream).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(maybe_message)) => {
                    maybe_message.err_tip(|| format!("Stream error at byte {}", self.bytes_received))
                }
                Poll::Ready(None) => Err(make_input_err!("Expected WriteRequest struct in stream")),
            }
        };

        // If we successfully got a message, update our internal state with the
        // message meta data.
        let maybe_message = match maybe_message {
            Ok(message) => {
                if self.message_count == 1 && self.second_msg.is_none() {
                    self.second_msg = Some(message.clone());
                }
                if self.message_count == 2 {
                    // Upon a successful third message, we discard the first
                    // and second messages.
                    self.first_msg.take();
                    self.second_msg.take();
                }
                self.write_finished = message.finish_write;
                self.bytes_received += message.data.len();
                self.message_count += 1;

                // Check that we haven't read past the expected end.
                if self.bytes_received > self.expected_size {
                    Err(make_input_err!(
                        "Sent too much data. Expected {}, but so far received {}",
                        self.expected_size,
                        self.bytes_received
                    ))
                } else {
                    Ok(message)
                }
            }
            error => error,
        };

        // Return the message.
        Poll::Ready(Some(maybe_message))
    }
}
