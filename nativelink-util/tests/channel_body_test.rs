// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use bytes::Bytes;
use futures::future;
use http_body_util::BodyExt;
use hyper::body::{Body, Frame};
use hyper::http::HeaderMap;
use nativelink_macro::nativelink_test;
use nativelink_util::channel_body::ChannelBody;
use nativelink_util::spawn;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::timeout;
use tonic::Status;

async fn setup_channel_body(
    frames: Vec<String>,
) -> (tokio::sync::mpsc::Sender<Frame<Bytes>>, ChannelBody) {
    let (tx, body) = ChannelBody::new();
    for frame in frames {
        tx.send(Frame::data(Bytes::from(frame))).await.unwrap();
    }
    (tx, body)
}

async fn check_frames(body: &mut ChannelBody, expected: Vec<String>) {
    for exp in expected {
        let frame = body.frame().await.unwrap().unwrap();
        assert_eq!(frame.into_data().unwrap(), Bytes::from(exp));
    }
    assert!(body.is_end_stream());
}

macro_rules! generate_channel_body_tests {
    ($($name:ident: $input:expr,)*) => {
        $(
            #[nativelink_test]
            async fn $name() {
                let (tx, mut body) = setup_channel_body($input.to_vec()).await;
                drop(tx);
                check_frames(&mut body, $input.to_vec()).await;
            }
        )*
    }
}

generate_channel_body_tests! {
    test_channel_body_empty: [String::new()],
    test_channel_body_basic: ["Hello".to_string()],
    test_channel_body_multiple_frames: ["Frame1".to_string(), "Frame2".to_string(), "Frame3".to_string()],
    test_channel_body_empty_frames: [String::new(), "NonEmpty".to_string(), String::new()],
    test_channel_body_large_data: [vec![0u8; 10 * 1024 * 1024].into_iter().map(|b| b.to_string()).collect::<String>()],
}

#[nativelink_test]
async fn test_channel_body_closure() {
    let (tx, mut body) = ChannelBody::new();
    tx.send(Frame::data(Bytes::from("Hello"))).await.unwrap();
    drop(tx);
    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), Bytes::from("Hello"));
    assert!(body.frame().await.is_none());
}

#[nativelink_test]
async fn test_channel_body_concurrent() {
    let (tx, body) = ChannelBody::new();

    let send_task = spawn!("send", async move {
        for i in 0..10 {
            tx.send(Frame::data(Bytes::from(format!("Frame{i}"))))
                .await
                .unwrap();
        }
    });

    let receive_task = spawn!("receive", async move {
        let mut body = body;
        for i in 0..10 {
            let frame = body.frame().await.unwrap().unwrap();
            assert_eq!(frame.into_data().unwrap(), Bytes::from(format!("Frame{i}")));
        }
    });

    let (send_result, receive_result) = future::join(send_task, receive_task).await;
    send_result.unwrap();
    receive_result.unwrap();
}

#[nativelink_test]
async fn test_channel_body_error_propagation() {
    let (tx, mut body) = ChannelBody::new();

    tx.send(Frame::data(Bytes::from("Error occurred")))
        .await
        .unwrap();
    drop(tx);

    let frame = body.frame().await.unwrap().unwrap();
    let data = frame.into_data().unwrap();
    assert_eq!(data, Bytes::from("Error occurred"));

    // Create a Status error from the received data
    let error = Status::internal(String::from_utf8(data.to_vec()).unwrap());
    assert_eq!(error.code(), tonic::Code::Internal);
    assert_eq!(error.message(), "Error occurred");
}

#[nativelink_test]
async fn test_channel_body_abrupt_end() {
    let (tx, mut body) = ChannelBody::new();

    tx.send(Frame::data(Bytes::from("Frame1"))).await.unwrap();
    tx.send(Frame::data(Bytes::from("Frame2"))).await.unwrap();

    drop(tx);

    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        Bytes::from("Frame1")
    );
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        Bytes::from("Frame2")
    );

    // Ensure that the stream has ended after dropping the sender
    assert!(body.frame().await.is_none());
}

#[nativelink_test]
async fn test_channel_body_trailers() {
    let (tx, mut body) = ChannelBody::new();

    tx.send(Frame::data(Bytes::from("Data"))).await.unwrap();

    let mut trailers = HeaderMap::new();
    trailers.insert("X-Trailer", "Value".parse().unwrap());
    tx.send(Frame::trailers(trailers.clone())).await.unwrap();

    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        Bytes::from("Data")
    );

    let received_trailers = body
        .frame()
        .await
        .unwrap()
        .unwrap()
        .into_trailers()
        .unwrap();
    assert_eq!(received_trailers, trailers);

    drop(tx);
    assert!(body.frame().await.is_none());
}

#[nativelink_test]
async fn test_channel_body_capacity() {
    let (tx, body) = ChannelBody::new();

    let send_task = spawn!("send", async move {
        for i in 0..40 {
            tx.send(Frame::data(Bytes::from(format!("Frame{i}"))))
                .await
                .unwrap();
        }
    });

    let receive_task = spawn!("receive", async move {
        let mut body = body;
        for i in 0..40 {
            let frame = body.frame().await.unwrap().unwrap();
            assert_eq!(frame.into_data().unwrap(), Bytes::from(format!("Frame{i}")));
        }
        assert!(body.frame().await.is_none());
    });

    let (send_result, receive_result) = future::join(send_task, receive_task).await;
    send_result.unwrap();
    receive_result.unwrap();
}

#[nativelink_test]
async fn test_channel_body_custom_buffer_size() {
    let (tx, mut body) = ChannelBody::with_buffer_size(5);

    for i in 0..5 {
        tx.send(Frame::data(Bytes::from(format!("Frame{i}"))))
            .await
            .unwrap();
    }

    // Attempt to send a frame when the buffer is full
    let send_result = tx.try_send(Frame::data(Bytes::from("Frame5")));
    assert!(send_result.is_err());

    for i in 0..5 {
        let frame = body.frame().await.unwrap().unwrap();
        assert_eq!(frame.into_data().unwrap(), Bytes::from(format!("Frame{i}")));
    }
}

#[nativelink_test]
async fn test_channel_body_is_end_stream() {
    let (tx, body) = ChannelBody::new();

    assert!(!body.is_end_stream());

    tx.send(Frame::data(Bytes::from("Data"))).await.unwrap();
    assert!(!body.is_end_stream());

    drop(tx);
    assert!(body.is_end_stream());
}

#[nativelink_test]
async fn test_channel_body_frame_types() {
    let (tx, mut body) = ChannelBody::new();

    tx.send(Frame::data(Bytes::from("Data"))).await.unwrap();

    let mut trailers = HeaderMap::new();
    trailers.insert("X-Trailer", "Value".parse().unwrap());
    tx.send(Frame::trailers(trailers.clone())).await.unwrap();

    let data_frame = body.frame().await.unwrap().unwrap();
    assert!(data_frame.is_data());
    assert_eq!(data_frame.into_data().unwrap(), Bytes::from("Data"));

    let trailer_frame = body.frame().await.unwrap().unwrap();
    assert!(trailer_frame.is_trailers());
    assert_eq!(trailer_frame.into_trailers().unwrap(), trailers);
}

#[nativelink_test]
async fn test_channel_body_error_scenarios() {
    let (tx, mut body) = ChannelBody::new();

    tx.send(Frame::data(Bytes::from("Data"))).await.unwrap();

    drop(tx);

    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), Bytes::from("Data"));

    // Ensure that the stream has ended after dropping the sender
    assert!(body.frame().await.is_none());
}

#[nativelink_test]
async fn test_channel_body_send_error() {
    let (tx, body) = ChannelBody::with_buffer_size(1);

    tx.send(Frame::data(Bytes::from("Data"))).await.unwrap();

    // Attempt to send when the buffer is full
    let result = tx.try_send(Frame::data(Bytes::from("More Data")));
    assert!(matches!(result, Err(TrySendError::Full(_))));

    drop(body);

    // Attempt to send after the receiver has been dropped
    let result = tx.try_send(Frame::data(Bytes::from("Even More Data")));
    assert!(matches!(result, Err(TrySendError::Closed(_))));
}

#[nativelink_test]
async fn test_channel_body_backpressure() {
    let (tx, mut body) = ChannelBody::with_buffer_size(2);

    // Fill the buffer
    tx.send(Frame::data(Bytes::from("Frame1"))).await.unwrap();
    tx.send(Frame::data(Bytes::from("Frame2"))).await.unwrap();

    // This should block because the buffer is full
    let send_task = spawn!("send", async move {
        tx.send(Frame::data(Bytes::from("Frame3"))).await.unwrap();
    });

    // Give some time for the send_task to block
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Read a frame, which should unblock the sender
    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), Bytes::from("Frame1"));

    // Wait for the send_task to complete
    send_task.await.unwrap();

    // Verify that all frames were received
    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), Bytes::from("Frame2"));
    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), Bytes::from("Frame3"));
}

#[nativelink_test]
async fn test_channel_body_cancellation() {
    let (tx, body) = ChannelBody::new();
    let tx_clone = tx.clone(); // Create a clone for later use

    let send_task = spawn!("send", async move {
        for i in 0..5 {
            match tx.send(Frame::data(Bytes::from(format!("Frame{i}")))).await {
                Ok(()) => {}
                Err(_) => break, // Stop if the receiver has been dropped
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    });

    let receive_task = spawn!("receive", async move {
        let mut body = body;
        for _ in 0..2 {
            match body.frame().await {
                Some(Ok(frame)) => {
                    println!("Received: {:?}", frame.into_data().unwrap());
                }
                _ => break,
            }
        }
        // Simulate cancellation by dropping the body
        drop(body);
    });

    let (send_result, receive_result) = future::join(send_task, receive_task).await;
    send_result.unwrap();
    receive_result.unwrap();

    // Use the cloned sender to check if the channel is closed
    assert!(tx_clone.is_closed());
}

#[nativelink_test]
async fn test_channel_body_timeout() {
    let (tx, mut body) = ChannelBody::new();

    // Try to receive a frame with a timeout
    let result = timeout(tokio::time::Duration::from_millis(100), body.frame()).await;
    assert!(result.is_err(), "Expected timeout error");

    // Send a frame
    tx.send(Frame::data(Bytes::from("Frame1"))).await.unwrap();

    // This should now succeed within the timeout
    let result = timeout(tokio::time::Duration::from_millis(100), body.frame()).await;
    assert!(
        result.is_ok(),
        "Expected frame to be received within timeout"
    );
    let frame = result.unwrap().unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), Bytes::from("Frame1"));

    // Close the channel
    drop(tx);

    // This should complete immediately with None, not timeout
    let result = timeout(tokio::time::Duration::from_millis(100), body.frame()).await;
    assert!(result.is_ok(), "Expected immediate completion");
    assert!(
        result.unwrap().is_none(),
        "Expected None as channel is closed"
    );
}

#[nativelink_test]
async fn test_channel_body_mixed_frame_types() {
    let (tx, mut body) = ChannelBody::new();

    let mut trailers1 = HeaderMap::new();
    trailers1.insert("X-Trailer-1", "Value1".parse().unwrap());
    let mut trailers2 = HeaderMap::new();
    trailers2.insert("X-Trailer-2", "Value2".parse().unwrap());

    // Send a mix of data frames and trailers
    tx.send(Frame::data(Bytes::from("Data1"))).await.unwrap();
    tx.send(Frame::trailers(trailers1.clone())).await.unwrap();
    tx.send(Frame::data(Bytes::from("Data2"))).await.unwrap();
    tx.send(Frame::trailers(trailers2.clone())).await.unwrap();
    tx.send(Frame::data(Bytes::from("Data3"))).await.unwrap();

    // Drop the sender to signal the end of the stream
    drop(tx);

    // Verify that frames are received in the correct order and type
    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), Bytes::from("Data1"));

    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_trailers().unwrap(), trailers1);

    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), Bytes::from("Data2"));

    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_trailers().unwrap(), trailers2);

    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), Bytes::from("Data3"));

    // Ensure the stream has ended
    assert!(body.frame().await.is_none());
}

#[nativelink_test]
async fn test_channel_body_small_buffer() {
    let (tx, mut body) = ChannelBody::with_buffer_size(1);

    // Test concurrent sending and receiving with a small buffer
    let send_task = spawn!("send", async move {
        for i in 0..5 {
            tx.send(Frame::data(Bytes::from(format!("Frame{i}"))))
                .await
                .unwrap();
        }
    });

    let receive_task = spawn!("receive", async move {
        for i in 0..5 {
            let frame = body.frame().await.unwrap().unwrap();
            assert_eq!(frame.into_data().unwrap(), Bytes::from(format!("Frame{i}")));
        }
        assert!(body.frame().await.is_none());
    });

    let (send_result, receive_result) = future::join(send_task, receive_task).await;
    send_result.unwrap();
    receive_result.unwrap();
}

#[nativelink_test]
async fn test_channel_body_large_buffer() {
    const BUFFER_SIZE: usize = 1_000_000; // 1 million frames
    const FRAME_COUNT: usize = 1_000_000;

    // Test with a very large buffer to ensure it can handle high volumes
    let (tx, mut body) = ChannelBody::with_buffer_size(BUFFER_SIZE);

    let send_task = spawn!("send", async move {
        for i in 0..FRAME_COUNT {
            tx.send(Frame::data(Bytes::from(format!("Frame{i}"))))
                .await
                .unwrap();
        }
    });

    let receive_task = spawn!("receive", async move {
        for i in 0..FRAME_COUNT {
            let frame = body.frame().await.unwrap().unwrap();
            assert_eq!(frame.into_data().unwrap(), Bytes::from(format!("Frame{i}")));
        }
        assert!(body.frame().await.is_none());
    });

    let (send_result, receive_result) = future::join(send_task, receive_task).await;
    send_result.unwrap();
    receive_result.unwrap();
}
