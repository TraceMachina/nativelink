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

use bytes::Bytes;
use futures::future;
use http_body_util::BodyExt;
use hyper::body::{Body, Frame};
use hyper::http::HeaderMap;
use nativelink_macro::nativelink_test;
use nativelink_util::channel_body_for_tests::ChannelBody;
use nativelink_util::spawn;
use tokio::sync::mpsc::error::TrySendError;

#[nativelink_test]
async fn test_channel_body_hello() {
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
