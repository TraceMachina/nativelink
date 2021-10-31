// Copyright 2021 Nathan (Blaise) Bruer.  All rights reserved.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use fast_async_mutex::mutex::Mutex;
use futures::{poll, FutureExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use async_fixed_buffer::AsyncFixedBuf;
use async_read_taker::{ArcMutexAsyncRead, AsyncReadTaker};
use error::{make_err, Code, Error};

#[cfg(test)]
mod async_read_taker_tests {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    #[tokio::test]
    async fn done_before_split() -> Result<(), Error> {
        let raw_fixed_buffer = AsyncFixedBuf::new(vec![0u8; 100].into_boxed_slice());
        let (rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        let mut taker = AsyncReadTaker::new(Arc::new(Mutex::new(Box::new(rx))), None::<Box<fn()>>, 1024);
        let write_data = vec![97u8; 50];
        {
            // Send our data.
            tx.write_all(&write_data).await?;
            tx.write(&vec![]).await?; // Write EOF.
        }
        {
            // Receive and check our data.
            let mut read_buffer = Vec::new();
            let read_sz = taker.read_to_end(&mut read_buffer).await?;
            assert_eq!(read_sz, 50, "Expected sizes to match");
            assert_eq!(&read_buffer, &write_data, "Expected sizes to match");
        }
        Ok(())
    }

    #[tokio::test]
    async fn done_fn_with_split() -> Result<(), Error> {
        let raw_fixed_buffer = AsyncFixedBuf::new(vec![0u8; 100].into_boxed_slice());
        let (rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        const WRITE_DATA: &[u8] = &[97u8; 50];
        const READ_AMOUNT: usize = 40;

        let reader: ArcMutexAsyncRead = Arc::new(Mutex::new(Box::new(rx)));
        let done = Arc::new(AtomicBool::new(false));
        {
            // Send our data.
            tx.write_all(&WRITE_DATA).await?;
            tx.write(&vec![]).await?; // Write EOF.
        }
        {
            // Receive first chunk and test our data.
            let done_clone = done.clone();
            let mut taker = AsyncReadTaker::new(
                reader.clone(),
                Some(move || done_clone.store(true, Ordering::Relaxed)),
                READ_AMOUNT,
            );

            let mut read_buffer = Vec::new();
            let read_sz = taker.read_to_end(&mut read_buffer).await?;
            assert_eq!(read_sz, READ_AMOUNT);
            assert_eq!(read_buffer.len(), READ_AMOUNT);
            assert_eq!(done.load(Ordering::Relaxed), false, "Should not be done");
            assert_eq!(&read_buffer, &WRITE_DATA[0..READ_AMOUNT]);
        }
        {
            // Receive last chunk and test our data.
            let done_clone = done.clone();
            let mut taker = AsyncReadTaker::new(
                reader.clone(),
                Some(move || done_clone.store(true, Ordering::Relaxed)),
                READ_AMOUNT,
            );

            let mut read_buffer = Vec::new();
            let read_sz = taker.read_to_end(&mut read_buffer).await?;
            const REMAINING_AMT: usize = WRITE_DATA.len() - READ_AMOUNT;
            assert_eq!(read_sz, REMAINING_AMT);
            assert_eq!(read_buffer.len(), REMAINING_AMT);
            assert_eq!(done.load(Ordering::Relaxed), true, "Should not be done");
            assert_eq!(&read_buffer, &WRITE_DATA[READ_AMOUNT..WRITE_DATA.len()]);
        }

        Ok(())
    }

    #[tokio::test]
    async fn shutdown_during_read() -> Result<(), Error> {
        let raw_fixed_buffer = AsyncFixedBuf::new(vec![0u8; 100].into_boxed_slice());
        let (rx, mut tx) = tokio::io::split(raw_fixed_buffer);

        const WRITE_DATA: &[u8] = &[97u8; 25];
        const READ_AMOUNT: usize = 50;

        let reader: ArcMutexAsyncRead = Arc::new(Mutex::new(Box::new(rx)));
        let done = Arc::new(AtomicBool::new(false));

        tx.write_all(&WRITE_DATA).await?;

        let done_clone = done.clone();
        let mut taker = Box::pin(AsyncReadTaker::new(
            reader.clone(),
            Some(move || done_clone.store(true, Ordering::Relaxed)),
            READ_AMOUNT,
        ));

        let mut read_buffer = Vec::new();
        let mut read_fut = taker.read_to_end(&mut read_buffer).boxed();
        {
            // Poll the future to make sure it did start reading. Failing to do this step makes this test useless.
            assert!(
                poll!(&mut read_fut).is_pending(),
                "Should not have received EOF. Should be pending"
            );
        }
        // Shutdown the sender. This should cause the futures to resolve.
        tx.shutdown().await?;
        {
            // Ensure an appropriate error message was returned.
            let err: Error = read_fut.await.unwrap_err().into();
            assert_eq!(err, make_err!(Code::Internal, "Sender disconnected"));
            assert_eq!(
                &read_buffer, &WRITE_DATA,
                "Expected poll!() macro to have processed the data we wrote"
            );
            assert_eq!(done.load(Ordering::Relaxed), false, "Should not have called done_fn");
        }

        Ok(())
    }
}
