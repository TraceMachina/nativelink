// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use async_fixed_buffer::AsyncFixedBuf;
use futures::{pin_mut, poll};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

use error::{make_err, Code, Error, ResultExt};

#[cfg(test)]
mod async_fixed_buffer_tests {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    #[tokio::test]
    async fn get_closer_closes_read_stream_early() -> Result<(), Error> {
        let mut raw_fixed_buffer = AsyncFixedBuf::new(vec![0u8; 32].into_boxed_slice());
        let stream_closer_fut = raw_fixed_buffer.get_closer();
        let (mut rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        tx.write_all(&vec![255u8; 4]).await?;

        let mut read_buffer = vec![0u8; 5];
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
        let mut raw_fixed_buffer = AsyncFixedBuf::new(vec![0u8; 4].into_boxed_slice());
        let stream_closer_fut = raw_fixed_buffer.get_closer();
        let (_, mut tx) = tokio::io::split(raw_fixed_buffer);

        let buffer = &vec![0u8; 5];
        let write_fut = tx.write_all(buffer);
        pin_mut!(write_fut);

        assert!(poll!(&mut write_fut).is_pending(), "Expecting to be pending");

        stream_closer_fut.await; // Now close the stream.

        let err: Error = write_fut.await.unwrap_err().into();
        assert_eq!(err, make_err!(Code::Internal, "Receiver disconnected"));
        Ok(())
    }

    #[tokio::test]
    async fn send_eof_closes_stream() -> Result<(), Error> {
        let raw_fixed_buffer = AsyncFixedBuf::new(vec![0u8; 32].into_boxed_slice());
        let (mut rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        let write_buffer = vec![0u8; 2];
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
        let raw_fixed_buffer = AsyncFixedBuf::new(vec![0u8; 32].into_boxed_slice());
        let (mut rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        let write_fut = async move {
            let write_buffer = vec![0u8; 2];
            tx.write_all(&write_buffer[..])
                .await
                .err_tip(|| "Failed to write_all")?;
            tx.flush().await.err_tip(|| "Failed to flush")
        };
        pin_mut!(write_fut);

        let mut read_buffer = vec![0u8; 1];

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
