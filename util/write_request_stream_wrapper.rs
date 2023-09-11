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

use futures::{Stream, StreamExt};

use error::{error_if, Error, ResultExt};
use proto::google::bytestream::WriteRequest;
use resource_info::ResourceInfo;

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

        let resource_info = ResourceInfo::new(&first_msg.resource_name).err_tip(|| {
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
            write_finished,
        })
    }

    pub async fn next(&mut self) -> Result<Option<WriteRequest>, Error> {
        if let Some(first_msg) = self.first_msg.take() {
            self.bytes_received += first_msg.data.len();
            return Ok(Some(first_msg));
        }
        if self.write_finished {
            error_if!(
                self.bytes_received != self.expected_size,
                "Did not send enough data. Expected {}, but so far received {}",
                self.expected_size,
                self.bytes_received
            );
            return Ok(None); // Previous message said it was the last msg.
        }
        error_if!(
            self.bytes_received > self.expected_size,
            "Sent too much data. Expected {}, but so far received {}",
            self.expected_size,
            self.bytes_received
        );
        let next_msg = self
            .stream
            .next()
            .await
            .err_tip(|| format!("Stream error at byte {}", self.bytes_received))?
            .err_tip(|| "Expected WriteRequest struct in stream")?;
        self.write_finished = next_msg.finish_write;
        self.bytes_received += next_msg.data.len();

        Ok(Some(next_msg))
    }
}
