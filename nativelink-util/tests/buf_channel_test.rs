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

use std::task::Poll;

use bytes::{Bytes, BytesMut};
use futures::poll;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_util::buf_channel::make_buf_channel_pair;
use pretty_assertions::assert_eq;
use tokio::try_join;

const DATA1: &str = "foo";
const DATA2: &str = "bar";
const DATA3: &str = "foobar1234";

#[nativelink_test]
async fn smoke_test() -> Result<(), Error> {
    let (mut tx, mut rx) = make_buf_channel_pair();
    tx.send(DATA1.into()).await?;
    tx.send(DATA2.into()).await?;
    assert_eq!(rx.recv().await?, DATA1);
    assert_eq!(rx.recv().await?, DATA2);
    Ok(())
}

#[nativelink_test]
async fn bytes_written_test() -> Result<(), Error> {
    let (mut tx, _rx) = make_buf_channel_pair();
    tx.send(DATA1.into()).await?;
    assert_eq!(tx.get_bytes_written(), DATA1.len() as u64);
    tx.send(DATA2.into()).await?;
    assert_eq!(tx.get_bytes_written(), (DATA1.len() + DATA2.len()) as u64);
    Ok(())
}

#[nativelink_test]
async fn sending_eof_sets_pipe_broken_test() -> Result<(), Error> {
    let (mut tx, mut rx) = make_buf_channel_pair();
    let tx_fut = async move {
        tx.send(DATA1.into()).await?;
        assert_eq!(tx.is_pipe_broken(), false);
        tx.send_eof()?;
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

#[nativelink_test]
async fn consume_all_test() -> Result<(), Error> {
    let (mut tx, mut rx) = make_buf_channel_pair();
    let tx_fut = async move {
        tx.send(DATA1.into()).await?;
        tx.send(DATA2.into()).await?;
        tx.send(DATA1.into()).await?;
        tx.send(DATA2.into()).await?;
        tx.send_eof()?;
        Result::<(), Error>::Ok(())
    };
    let rx_fut = async move {
        assert_eq!(
            rx.consume(None).await?,
            Bytes::from(format!("{DATA1}{DATA2}{DATA1}{DATA2}"))
        );
        Result::<(), Error>::Ok(())
    };
    try_join!(tx_fut, rx_fut)?;
    Ok(())
}

/// Test to ensure data is optimized so that the exact same pointer is received
/// when calling `collect_all_with_size_hint` when a copy is not needed.
#[nativelink_test]
async fn consume_all_is_optimized_test() -> Result<(), Error> {
    let (mut tx, mut rx) = make_buf_channel_pair();
    let sent_data = Bytes::from(DATA1);
    let send_data_ptr = sent_data.as_ptr();
    let tx_fut = async move {
        tx.send(sent_data).await?;
        tx.send_eof()?;
        Result::<(), Error>::Ok(())
    };
    let rx_fut = async move {
        // Because data is 1 chunk and an EOF, we should not need to copy
        // and should get the exact same pointer.
        let received_data = rx.consume(None).await?;
        assert_eq!(received_data.as_ptr(), send_data_ptr);
        Result::<(), Error>::Ok(())
    };
    try_join!(tx_fut, rx_fut)?;
    Ok(())
}

#[nativelink_test]
async fn consume_some_test() -> Result<(), Error> {
    let (mut tx, mut rx) = make_buf_channel_pair();
    let tx_fut = async move {
        tx.send(DATA1.into()).await?;
        tx.send(DATA2.into()).await?;
        tx.send(DATA1.into()).await?;
        tx.send(DATA2.into()).await?;
        tx.send_eof()?;
        Result::<(), Error>::Ok(())
    };
    let rx_fut = async move {
        let all_data = Bytes::from(format!("{DATA1}{DATA2}{DATA1}{DATA2}"));
        assert_eq!(rx.consume(Some(1)).await?, all_data.slice(0..1));
        assert_eq!(rx.consume(Some(3)).await?, all_data.slice(1..4));
        assert_eq!(rx.consume(Some(4)).await?, all_data.slice(4..8));
        // Last chunk take too much data and expect EOF to be hit.
        assert_eq!(rx.consume(Some(100)).await?, all_data.slice(8..12));
        Result::<(), Error>::Ok(())
    };
    try_join!(tx_fut, rx_fut)?;
    Ok(())
}

/// This test ensures that when we are taking just one message in the stream,
/// we don't need to concat the data together and instead return a view to
/// the original data instead of making a copy.
#[nativelink_test]
async fn consume_some_optimized_test() -> Result<(), Error> {
    let (mut tx, mut rx) = make_buf_channel_pair();
    let first_chunk = Bytes::from(DATA1);
    let first_chunk_ptr = first_chunk.as_ptr();
    let tx_fut = async move {
        tx.send(first_chunk).await?;
        tx.send_eof()?;
        Result::<(), Error>::Ok(())
    };
    let rx_fut = async move {
        assert_eq!(rx.consume(Some(1)).await?.as_ptr(), first_chunk_ptr);
        assert_eq!(rx.consume(Some(100)).await?.as_ptr(), unsafe {
            first_chunk_ptr.add(1)
        });
        Result::<(), Error>::Ok(())
    };
    try_join!(tx_fut, rx_fut)?;
    Ok(())
}

#[nativelink_test]
async fn consume_some_reads_eof() -> Result<(), Error> {
    let (mut tx, mut rx) = make_buf_channel_pair();
    tx.send(DATA1.into()).await?;

    let consume_fut = rx.consume(Some(DATA1.len()));
    tokio::pin!(consume_fut);
    assert_eq!(
        poll!(&mut consume_fut),
        Poll::Pending,
        "Consume should not have completed yet"
    );
    tx.send_eof()?;
    assert_eq!(consume_fut.await?, Bytes::from(DATA1));
    Ok(())
}

#[nativelink_test]
async fn simple_stream_test() -> Result<(), Error> {
    use futures::StreamExt;
    let (mut tx, mut rx) = make_buf_channel_pair();
    let tx_fut = async move {
        tx.send(DATA1.into()).await?;
        tx.send(DATA2.into()).await?;
        tx.send(DATA1.into()).await?;
        tx.send(DATA2.into()).await?;
        tx.send_eof()?;
        Result::<(), Error>::Ok(())
    };
    let rx_fut = async move {
        assert_eq!(
            rx.next().await.map(|v| v.err_tip(|| "")),
            Some(Ok(Bytes::from(DATA1)))
        );
        assert_eq!(
            rx.next().await.map(|v| v.err_tip(|| "")),
            Some(Ok(Bytes::from(DATA2)))
        );
        assert_eq!(
            rx.next().await.map(|v| v.err_tip(|| "")),
            Some(Ok(Bytes::from(DATA1)))
        );
        assert_eq!(
            rx.next().await.map(|v| v.err_tip(|| "")),
            Some(Ok(Bytes::from(DATA2)))
        );
        assert_eq!(rx.next().await.map(|v| v.err_tip(|| "")), None);
        Result::<(), Error>::Ok(())
    };
    try_join!(tx_fut, rx_fut)?;
    Ok(())
}

#[nativelink_test]
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
                    tx.send_eof()?;
                    Result::<(), Error>::Ok(())
                };
                let rx_fut = async move {
                    let mut round_trip_data = BytesMut::new();
                    for _ in (0..data_size).step_by(read_size) {
                        round_trip_data.extend(rx.consume(Some(read_size)).await?.iter());
                    }
                    assert_eq!(round_trip_data.freeze(), expected_data);
                    rx.drain().await?;
                    Result::<(), Error>::Ok(())
                };
                try_join!(tx_fut, rx_fut)?;
            }
        }
    }
    Ok(())
}

#[nativelink_test]
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
            Err(make_err!(
                Code::Internal,
                "Sender dropped before sending EOF"
            ))
        );
        Result::<(), Error>::Ok(())
    };
    try_join!(tx_fut, rx_fut)?;
    Ok(())
}

#[nativelink_test]
async fn bind_bufferd_test() -> Result<(), Error> {
    let (mut tx_source, mut rx_bind) = make_buf_channel_pair();
    let (mut tx_bind, mut rx_final) = make_buf_channel_pair();
    try_join!(
        async move {
            let result = tx_bind.bind_bufferd(&mut rx_bind).await;
            assert!(result.is_err(), "Should be error, got: {result:?}");
            assert!(result
                .err()
                .unwrap()
                .to_string()
                .contains("Sender dropped before sending EOF"));
            Ok(())
        },
        async move {
            tx_source.send(DATA1.into()).await.unwrap();
            drop(tx_source);
            assert_eq!(
                rx_final.recv().await,
                Err(make_err!(
                    Code::Internal,
                    "Sender dropped before sending EOF"
                ))
            );
            Result::<_, Error>::Ok(())
        }
    )
    .unwrap();
    Ok(())
}
