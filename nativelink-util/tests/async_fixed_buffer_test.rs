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

use std::sync::Arc;
use std::task::Poll;

use futures::{pin_mut, poll};
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_util::async_fixed_buffer::AsyncFixedBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cfg(test)]
mod async_fixed_buffer_tests {
    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    #[tokio::test]
    async fn ensure_cross_thread_support() -> Result<(), Error> {
        let raw_fixed_buffer = AsyncFixedBuf::<32>::new();
        let (mut rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        const WRITE_SIZE: usize = 50;
        let write_buf = Arc::new(vec![88u8; WRITE_SIZE]);
        let write_buf_clone = write_buf.clone();
        let spawn_fut = tokio::spawn(async move {
            tx.write_all(write_buf_clone.as_ref())
                .await
                .err_tip(|| "Failed to write_all")?;
            tx.write(&[]).await.err_tip(|| "Could not write EOF")?; // Write EOF.
            Result::<(), Error>::Ok(())
        });
        pin_mut!(spawn_fut);

        const READ_SIZE1: usize = 24;
        {
            let mut read_buf = vec![0u8; READ_SIZE1];
            let bytes_read = rx.read_exact(&mut read_buf).await?;
            assert_eq!(bytes_read, READ_SIZE1, "Expected {} bytes read", READ_SIZE1);

            tokio::task::yield_now().await;

            assert!(
                poll!(&mut spawn_fut).is_pending(),
                "Write should not be done yet because not enough buffer space yet"
            );
            assert_eq!(&read_buf, &write_buf.as_ref()[0..READ_SIZE1], "Expected data to match");
        }
        const READ_SIZE2: usize = 10;
        {
            let mut read_buf = vec![0u8; READ_SIZE2];
            let bytes_read = rx.read_exact(&mut read_buf).await?;
            assert_eq!(bytes_read, READ_SIZE2, "Expected {} bytes read", READ_SIZE2);

            tokio::task::yield_now().await;

            let spawn_result_poll = poll!(&mut spawn_fut)?;
            assert_eq!(
                spawn_result_poll,
                Poll::Ready(Ok(())),
                "Expected result of spawn to be success"
            );
            assert_eq!(
                &read_buf[..],
                &write_buf.as_ref()[READ_SIZE1..(READ_SIZE1 + READ_SIZE2)],
                "Expected data to match"
            );
        }
        const READ_SIZE3: usize = READ_SIZE1 + READ_SIZE2;
        {
            let mut read_buf = Vec::new();
            let first_read = rx
                .read_to_end(&mut read_buf)
                .await
                .err_tip(|| "Failed to read_to_end")?;
            assert_eq!(first_read, WRITE_SIZE - READ_SIZE3, "Expected read_to_end to match");
            assert_eq!(
                &read_buf[..],
                &write_buf.as_ref()[READ_SIZE3..WRITE_SIZE],
                "Expected data to match"
            );
        }

        Ok(())
    }

    #[tokio::test]
    // In early development a bug was found where if a future was dropped the subsequent future
    // would would never complete.
    async fn check_dropped_futures() -> Result<(), Error> {
        let raw_fixed_buffer = AsyncFixedBuf::<32>::new();
        let (mut rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        tx.write_all(&[255u8; 5]).await?;
        {
            let mut read_buf = vec![0u8; 20];
            let fut = rx.read_to_end(&mut read_buf);
            pin_mut!(fut);
            assert!(
                poll!(fut).is_pending(),
                "Expected poll of read buffer to be pending as eof not sent yet"
            );
        }
        // The read_exact future should be dropped here.
        let mut read_buf = Vec::new();
        let read_fut = {
            let mut fut = Box::pin(rx.read_to_end(&mut read_buf));
            assert!(
                poll!(&mut fut).is_pending(),
                "Expected next read to also be pending (still no data sent yet)"
            );
            fut
        };

        let write_buf = vec![88u8; 2];
        tx.write_all(&write_buf).await.err_tip(|| "Could not write_all")?;
        tx.write(&[]).await.err_tip(|| "Could not write EOF")?;

        let len = read_fut.await.err_tip(|| "Could not finish read_exact")?;
        assert_eq!(len, write_buf.len(), "Expected read amount to match write amount");
        assert_eq!(&read_buf, &write_buf, "Expected data to match");

        Ok(())
    }

    #[tokio::test]
    async fn get_closer_closes_read_stream_early() -> Result<(), Error> {
        let mut raw_fixed_buffer = AsyncFixedBuf::<32>::new();
        let stream_closer_fut = raw_fixed_buffer.get_closer();
        let (mut rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        tx.write_all(&[255u8; 4]).await?;

        let mut read_buffer = [0u8; 5];
        let read_fut = rx.read_exact(&mut read_buffer[..]);
        pin_mut!(read_fut);

        assert!(poll!(&mut read_fut).is_pending(), "Expecting to be pending");

        stream_closer_fut.await; // Now close the stream.

        let err: Error = read_fut.await.unwrap_err().into();
        assert_eq!(err, make_err!(Code::Internal, "Sender disconnected"));
        Ok(())
    }

    #[tokio::test]
    async fn get_closer_closes_write_stream_early() -> Result<(), Error> {
        let mut raw_fixed_buffer = AsyncFixedBuf::<4>::new();
        let stream_closer_fut = raw_fixed_buffer.get_closer();
        let (_, mut tx) = tokio::io::split(raw_fixed_buffer);

        let buffer = vec![0u8; 5];
        let write_fut = tx.write_all(&buffer);
        pin_mut!(write_fut);

        assert!(poll!(&mut write_fut).is_pending(), "Expecting to be pending");

        stream_closer_fut.await; // Now close the stream.

        let err: Error = write_fut.await.unwrap_err().into();
        assert_eq!(err, make_err!(Code::Internal, "Receiver disconnected"));
        Ok(())
    }

    #[tokio::test]
    async fn send_eof_closes_stream() -> Result<(), Error> {
        let raw_fixed_buffer = AsyncFixedBuf::<32>::new();
        let (mut rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        let write_buffer = [0u8; 2];
        tx.write_all(&write_buffer[..])
            .await
            .err_tip(|| "Failed to write_all")?;

        let mut read_buffer = vec![0u8; 64];
        let read_all_fut = rx.read_to_end(&mut read_buffer);
        pin_mut!(read_all_fut);

        assert!(poll!(&mut read_all_fut).is_pending(), "Expecting to be pending");
        tx.write(&[]).await.err_tip(|| "Failed to write eof")?; // Now send EOF
        assert!(poll!(&mut read_all_fut).is_ready(), "Expecting to be ready");

        Ok(())
    }

    #[tokio::test]
    async fn flush_smoke_test() -> Result<(), Error> {
        let raw_fixed_buffer = AsyncFixedBuf::<32>::new();
        let (mut rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        let write_fut = async move {
            let write_buffer = [0u8; 2];
            tx.write_all(&write_buffer[..])
                .await
                .err_tip(|| "Failed to write_all")?;
            tx.flush().await.err_tip(|| "Failed to flush")
        };
        pin_mut!(write_fut);

        let mut read_buffer = [0u8; 1];

        assert!(poll!(&mut write_fut).is_pending(), "Expecting to be pending");
        assert_eq!(
            rx.read_exact(&mut read_buffer[..]).await?,
            1,
            "Should have read one byte"
        );

        assert!(poll!(&mut write_fut).is_pending(), "Expecting to still be pending");
        assert_eq!(
            rx.read_exact(&mut read_buffer[..]).await?,
            1,
            "Should have read one byte"
        );

        assert!(poll!(&mut write_fut).is_ready(), "Expecting to be ready");

        Ok(())
    }
}
