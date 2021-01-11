// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use async_fixed_buffer::AsyncFixedBuf;
use error::{make_err, Code, Error, ResultExt};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

#[cfg(test)]
mod async_fixed_buffer_tests {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    #[tokio::test]
    async fn get_closer_closes_read_stream_early() -> Result<(), Error> {
        let mut raw_fixed_buffer = AsyncFixedBuf::new(vec![0u8; 32].into_boxed_slice());
        let mut stream_closer = raw_fixed_buffer.get_closer();
        let (mut rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        tx.write_all(&vec![255u8; 4]).await?;

        let read_spawn = tokio::spawn(async move {
            let mut read_buffer = vec![0u8; 5];
            rx.read_exact(&mut read_buffer[..]).await
        });
        // Wait a few cycles to ensure we are in a read loop.
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        stream_closer();
        let read_result = read_spawn.await.err_tip(|| "Failed to join thread")?;
        let err: Error = read_result.unwrap_err().into();
        assert_eq!(err, make_err!(Code::Internal, "Sender disconnected"));
        Ok(())
    }

    #[tokio::test]
    async fn get_closer_closes_write_stream_early() -> Result<(), Error> {
        let mut raw_fixed_buffer = AsyncFixedBuf::new(vec![0u8; 4].into_boxed_slice());
        let mut stream_closer = raw_fixed_buffer.get_closer();
        let (_, mut tx) = tokio::io::split(raw_fixed_buffer);

        let write_spawn = tokio::spawn(async move {
            let mut read_buffer = vec![0u8; 5];
            tx.write_all(&mut read_buffer[..]).await
        });
        // Wait a few cycles to ensure we are in a read loop.
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        stream_closer();
        let read_result = write_spawn.await.err_tip(|| "Failed to join thread")?;
        let err: Error = read_result.unwrap_err().into();
        assert_eq!(err, make_err!(Code::Internal, "Receiver disconnected"));
        Ok(())
    }
}
