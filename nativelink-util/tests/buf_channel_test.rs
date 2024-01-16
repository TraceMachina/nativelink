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

use bytes::{Bytes, BytesMut};
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_util::buf_channel::make_buf_channel_pair;
use tokio::try_join;

#[cfg(test)]
mod buf_channel_tests {
    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    const DATA1: &str = "foo";
    const DATA2: &str = "bar";
    const DATA3: &str = "foobar1234";

    #[tokio::test]
    async fn smoke_test() -> Result<(), Error> {
        let (mut tx, mut rx) = make_buf_channel_pair();
        tx.send(DATA1.into()).await?;
        tx.send(DATA2.into()).await?;
        assert_eq!(rx.recv().await?, DATA1);
        assert_eq!(rx.recv().await?, DATA2);
        Ok(())
    }

    #[tokio::test]
    async fn bytes_written_test() -> Result<(), Error> {
        let (mut tx, _rx) = make_buf_channel_pair();
        tx.send(DATA1.into()).await?;
        assert_eq!(tx.get_bytes_written(), DATA1.len() as u64);
        tx.send(DATA2.into()).await?;
        assert_eq!(tx.get_bytes_written(), (DATA1.len() + DATA2.len()) as u64);
        Ok(())
    }

    #[tokio::test]
    async fn sending_eof_sets_pipe_broken_test() -> Result<(), Error> {
        let (mut tx, mut rx) = make_buf_channel_pair();
        let tx_fut = async move {
            tx.send(DATA1.into()).await?;
            assert_eq!(tx.is_pipe_broken(), false);
            tx.send_eof().await?;
            assert_eq!(tx.is_pipe_broken(), true);
            Result::<(), Error>::Ok(())
        };
        let rx_fut = async move {
            assert_eq!(rx.recv().await?, Bytes::from(DATA1));
            assert_eq!(rx.recv().await?, Bytes::new());
            Result::<(), Error>::Ok(())
        };
        try_join!(tx_fut, rx_fut)?;
        Ok(())
    }

    #[tokio::test]
    async fn rx_closes_before_eof_sends_err_to_tx_test() -> Result<(), Error> {
        let (mut tx, mut rx) = make_buf_channel_pair();
        let tx_fut = async move {
            // Send one message.
            tx.send(DATA1.into()).await?;
            // Try to send EOF, but expect error because receiver will be dropped without taking it.
            assert_eq!(
                tx.send_eof().await,
                Err(make_err!(Code::Internal, "Receiver went away before receiving EOF"))
            );
            Result::<(), Error>::Ok(())
        };
        let rx_fut = async move {
            // Receive first message.
            assert_eq!(rx.recv().await?, Bytes::from(DATA1));
            // Now drop rx without receiving EOF.
            Result::<(), Error>::Ok(())
        };
        try_join!(tx_fut, rx_fut)?;
        Ok(())
    }

    #[tokio::test]
    async fn set_close_after_size_test() -> Result<(), Error> {
        let (mut tx, mut rx) = make_buf_channel_pair();
        let tx_fut = async move {
            tx.send(DATA1.into()).await?;
            tx.send_eof().await?;
            Result::<(), Error>::Ok(())
        };
        let rx_fut = async move {
            rx.set_close_after_size(DATA1.len() as u64);
            assert_eq!(rx.recv().await?, Bytes::from(DATA1));
            // Now there's an EOF, but we are going to drop instead of taking it.
            // We should not send an error to the tx.
            Result::<(), Error>::Ok(())
        };
        try_join!(tx_fut, rx_fut)?;
        Ok(())
    }

    #[tokio::test]
    async fn collect_all_with_size_hint_test() -> Result<(), Error> {
        let (mut tx, rx) = make_buf_channel_pair();
        let tx_fut = async move {
            tx.send(DATA1.into()).await?;
            tx.send(DATA2.into()).await?;
            tx.send(DATA1.into()).await?;
            tx.send(DATA2.into()).await?;
            tx.send_eof().await?;
            Result::<(), Error>::Ok(())
        };
        let rx_fut = async move {
            assert_eq!(
                rx.collect_all_with_size_hint(0).await?,
                Bytes::from(format!("{}{}{}{}", DATA1, DATA2, DATA1, DATA2))
            );
            Result::<(), Error>::Ok(())
        };
        try_join!(tx_fut, rx_fut)?;
        Ok(())
    }

    /// Test to ensure data is optimized so that the exact same pointer is received
    /// when calling `collect_all_with_size_hint` when a copy is not needed.
    #[tokio::test]
    async fn collect_all_with_size_hint_is_optimized_test() -> Result<(), Error> {
        let (mut tx, rx) = make_buf_channel_pair();
        let sent_data = Bytes::from(DATA1);
        let send_data_ptr = sent_data.as_ptr();
        let tx_fut = async move {
            tx.send(sent_data).await?;
            tx.send_eof().await?;
            Result::<(), Error>::Ok(())
        };
        let rx_fut = async move {
            // Because data is 1 chunk and an EOF, we should not need to copy
            // and should get the exact same pointer.
            let received_data = rx.collect_all_with_size_hint(0).await?;
            assert_eq!(received_data.as_ptr(), send_data_ptr);
            Result::<(), Error>::Ok(())
        };
        try_join!(tx_fut, rx_fut)?;
        Ok(())
    }

    #[tokio::test]
    async fn take_test() -> Result<(), Error> {
        let (mut tx, mut rx) = make_buf_channel_pair();
        let tx_fut = async move {
            tx.send(DATA1.into()).await?;
            tx.send(DATA2.into()).await?;
            tx.send(DATA1.into()).await?;
            tx.send(DATA2.into()).await?;
            tx.send_eof().await?;
            Result::<(), Error>::Ok(())
        };
        let rx_fut = async move {
            let all_data = Bytes::from(format!("{}{}{}{}", DATA1, DATA2, DATA1, DATA2));
            assert_eq!(rx.take(1).await?, all_data.slice(0..1));
            assert_eq!(rx.take(3).await?, all_data.slice(1..4));
            assert_eq!(rx.take(4).await?, all_data.slice(4..8));
            // Last chunk take too much data and expect EOF to be hit.
            assert_eq!(rx.take(100).await?, all_data.slice(8..12));
            Result::<(), Error>::Ok(())
        };
        try_join!(tx_fut, rx_fut)?;
        Ok(())
    }

    /// This test ensures that when we are taking just one message in the stream,
    /// we don't need to concat the data together and instead return a view to
    /// the original data instead of making a copy.
    #[tokio::test]
    async fn take_optimized_test() -> Result<(), Error> {
        let (mut tx, mut rx) = make_buf_channel_pair();
        let first_chunk = Bytes::from(DATA1);
        let first_chunk_ptr = first_chunk.as_ptr();
        let tx_fut = async move {
            tx.send(first_chunk).await?;
            tx.send_eof().await?;
            Result::<(), Error>::Ok(())
        };
        let rx_fut = async move {
            assert_eq!(rx.take(1).await?.as_ptr(), first_chunk_ptr);
            assert_eq!(rx.take(100).await?.as_ptr(), unsafe { first_chunk_ptr.add(1) });
            Result::<(), Error>::Ok(())
        };
        try_join!(tx_fut, rx_fut)?;
        Ok(())
    }

    #[tokio::test]
    async fn simple_stream_test() -> Result<(), Error> {
        use futures::StreamExt;
        let (mut tx, mut rx) = make_buf_channel_pair();
        let tx_fut = async move {
            tx.send(DATA1.into()).await?;
            tx.send(DATA2.into()).await?;
            tx.send(DATA1.into()).await?;
            tx.send(DATA2.into()).await?;
            tx.send_eof().await?;
            Result::<(), Error>::Ok(())
        };
        let rx_fut = async move {
            assert_eq!(rx.next().await.map(|v| v.err_tip(|| "")), Some(Ok(Bytes::from(DATA1))));
            assert_eq!(rx.next().await.map(|v| v.err_tip(|| "")), Some(Ok(Bytes::from(DATA2))));
            assert_eq!(rx.next().await.map(|v| v.err_tip(|| "")), Some(Ok(Bytes::from(DATA1))));
            assert_eq!(rx.next().await.map(|v| v.err_tip(|| "")), Some(Ok(Bytes::from(DATA2))));
            assert_eq!(rx.next().await.map(|v| v.err_tip(|| "")), None);
            Result::<(), Error>::Ok(())
        };
        try_join!(tx_fut, rx_fut)?;
        Ok(())
    }

    #[tokio::test]
    async fn send_and_take_fuzz_test() -> Result<(), Error> {
        const DATA3_END_POS: usize = DATA3.len() + 1;
        for data_size in 1..DATA3_END_POS {
            let data: Vec<u8> = DATA3.as_bytes()[0..data_size].to_vec();

            for write_size in 1..DATA3_END_POS {
                for read_size in 1..DATA3_END_POS {
                    let tx_data = Bytes::from(data.clone());
                    let expected_data = Bytes::from(data.clone());

                    let (mut tx, mut rx) = make_buf_channel_pair();

                    let tx_fut = async move {
                        for i in (0..data_size).step_by(write_size) {
                            tx.send(tx_data.slice(i..std::cmp::min(data_size, i + write_size)))
                                .await?;
                        }
                        tx.send_eof().await?;
                        Result::<(), Error>::Ok(())
                    };
                    let rx_fut = async move {
                        let mut round_trip_data = BytesMut::new();
                        for _ in (0..data_size).step_by(read_size) {
                            round_trip_data.extend(rx.take(read_size).await?.iter());
                        }
                        assert_eq!(round_trip_data.freeze(), expected_data);
                        Result::<(), Error>::Ok(())
                    };
                    try_join!(tx_fut, rx_fut)?;
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn rx_gets_error_if_tx_drops_test() -> Result<(), Error> {
        let (mut tx, mut rx) = make_buf_channel_pair();
        let tx_fut = async move {
            tx.send(DATA1.into()).await?;
            Result::<(), Error>::Ok(())
        };
        let rx_fut = async move {
            assert_eq!(rx.recv().await?, Bytes::from(DATA1));
            assert_eq!(
                rx.recv().await,
                Err(make_err!(Code::Internal, "Received erroneous partial chunk: Error {{ code: Internal, messages: [\"Writer was dropped before EOF was sent\"] }}"))
            );
            Result::<(), Error>::Ok(())
        };
        try_join!(tx_fut, rx_fut)?;
        Ok(())
    }

    #[tokio::test]
    async fn rx_accepts_tx_drop_test_when_eof_ignored() -> Result<(), Error> {
        let (mut tx, mut rx) = make_buf_channel_pair();
        tx.set_ignore_eof();
        let tx_fut = async move {
            tx.send(DATA1.into()).await?;
            Result::<(), Error>::Ok(())
        };
        let rx_fut = async move {
            assert_eq!(rx.recv().await?, Bytes::from(DATA1));
            assert_eq!(
                rx.recv().await,
                Err(make_err!(Code::Internal, "Failed to send closing ok message to write"))
            );
            Result::<(), Error>::Ok(())
        };
        try_join!(tx_fut, rx_fut)?;
        Ok(())
    }
}
