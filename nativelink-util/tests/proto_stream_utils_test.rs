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

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use nativelink_error::Error;
use nativelink_proto::google::bytestream::WriteRequest;
use nativelink_util::common::DigestInfo;
use nativelink_util::proto_stream_utils::{
    WriteRequestStreamWrapper, WriteState, WriteStateWrapper,
};
use parking_lot::Mutex;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(test)]
mod proto_stream_utils_tests {
    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    const INSTANCE_NAME: &str = "test-instance";

    // Regression test for TraceMachina/nativelink#745.
    #[tokio::test]
    async fn ensure_no_errors_if_only_first_message_has_resource_name_set() -> Result<(), Error> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<WriteRequest, Error>>();

        const RAW_DATA: &str = "thisdatafoo";
        const DIGEST: DigestInfo = DigestInfo::new([0u8; 32], RAW_DATA.len() as i64);
        let message1 = WriteRequest {
            resource_name: format!(
                "{INSTANCE_NAME}/uploads/some-uuid/blobs/{}/{}",
                DIGEST.hash_str(),
                DIGEST.size_bytes
            ),
            write_offset: 0,
            finish_write: false,
            data: Bytes::from_static(RAW_DATA[..4].as_bytes()),
        };
        let message2 = WriteRequest {
            resource_name: String::new(),
            write_offset: 4,
            finish_write: false,
            data: Bytes::from_static(RAW_DATA[4..8].as_bytes()),
        };
        let message3 = WriteRequest {
            resource_name: String::new(),
            write_offset: 8,
            finish_write: true,
            data: Bytes::from_static(RAW_DATA[8..].as_bytes()),
        };

        {
            tx.send(Ok(message1.clone())).unwrap();
            tx.send(Ok(message2.clone())).unwrap();
            tx.send(Ok(message3.clone())).unwrap();
            drop(tx); // Close the channel.
        }

        let local_state = Arc::new(Mutex::new(WriteState::new(
            INSTANCE_NAME.to_string(),
            WriteRequestStreamWrapper::from(UnboundedReceiverStream::new(rx)).await?,
        )));
        let mut write_state_wrapper = WriteStateWrapper::new(local_state.clone());

        {
            // Ensure we transported our data properly.
            assert_eq!(write_state_wrapper.next().await, Some(message1));
            assert_eq!(write_state_wrapper.next().await, Some(message2));
            assert_eq!(write_state_wrapper.next().await, Some(message3));
            assert_eq!(write_state_wrapper.next().await, None);

            // Ensure no stream errors were set.
            assert_eq!(local_state.lock().take_read_stream_error(), None);
        }

        Ok(())
    }
}
