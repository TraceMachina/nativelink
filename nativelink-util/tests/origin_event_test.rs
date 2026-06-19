// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::pin::Pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use std::sync::Arc;

use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_proto::com::github::trace_machina::nativelink::events::{
    Event, OriginEvent, RequestEvent, ResponseEvent, StreamEvent, event, request_event,
    response_event, stream_event,
};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::default_health_status_indicator;
use nativelink_util::health_utils::HealthStatusIndicator;
use nativelink_util::origin_event::get_id_for_event;
use nativelink_util::origin_event_publisher::OriginEventPublisher;
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, UploadSizeInfo,
};
use tokio::sync::{broadcast, mpsc};
use tokio::time::sleep;
use tonic::async_trait;

/// A store whose `update` fails the first `fail_until` calls, then succeeds —
/// to verify the origin-event publisher retries a transient upload failure
/// instead of dropping the events (the resource-sizing durability gap).
#[derive(Debug, MetricsComponent)]
struct FlakyStore {
    fail_until: usize,
    attempts: AtomicUsize,
}

#[async_trait]
#[allow(clippy::todo)]
impl StoreDriver for FlakyStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        _keys: &[StoreKey<'_>],
        _results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        todo!();
    }

    async fn update(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<u64, Error> {
        // Drain so the writer side completes.
        reader.consume(None).await?;
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= self.fail_until {
            return Err(make_err!(
                Code::Unavailable,
                "flaky store failure {attempt}"
            ));
        }
        Ok(0)
    }

    async fn get_part(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        todo!();
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(
        self: Arc<Self>,
        _callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        todo!();
    }
}

default_health_status_indicator!(FlakyStore);

// Regression test for the action-level resource-sizing durability gap: a
// transient store-upload failure (e.g. a Redis Sentinel failover) must NOT drop
// origin events — the publisher retries until the upload succeeds.
#[nativelink_test]
async fn origin_event_publisher_retries_store_upload() {
    let store = Arc::new(FlakyStore {
        fail_until: 2,
        attempts: AtomicUsize::new(0),
    });
    let (tx, rx) = mpsc::channel::<OriginEvent>(16);
    let (shutdown_tx, _shutdown_rx) = broadcast::channel(1);
    let publisher = OriginEventPublisher::new(Store::new(store.clone()), rx, shutdown_tx);
    let task = tokio::spawn(publisher.run());

    tx.send(OriginEvent {
        version: 0,
        event_id: "e1".to_string(),
        parent_event_id: String::new(),
        bazel_request_metadata: None,
        identity: String::new(),
        event: None,
    })
    .await
    .unwrap();

    // 2 transient failures + 1 success = 3 attempts.
    for _ in 0..100 {
        if store.attempts.load(Ordering::SeqCst) >= 3 {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        store.attempts.load(Ordering::SeqCst),
        3,
        "publisher must retry the upload past transient failures instead of dropping events",
    );
    task.abort();
}

macro_rules! event_assert {
    ($event:ident, $val:expr) => {
        assert_eq!(
            get_expected_value(&$event),
            get_id_for_event(&$event),
            "Incorrect event id for {}",
            stringify!($val)
        );
    };
}

macro_rules! test_event {
    (Request, None) => {
        let event = Event {
            event: Some(event::Event::Request(RequestEvent { event: None })),
        };
        event_assert!(event, Request(None));
    };
    (Request, $enum_type:ident) => {
        let event = Event {
            event: Some(event::Event::Request(RequestEvent {
                event: Some(request_event::Event::$enum_type(Default::default())),
            })),
        };
        event_assert!(event, Request($enum_type));
    };
    (Response, None) => {
        let event = Event {
            event: Some(event::Event::Response(ResponseEvent { event: None })),
        };
        event_assert!(event, Response(None));
    };
    (Response, $enum_type:ident) => {
        let event = Event {
            event: Some(event::Event::Response(ResponseEvent {
                event: Some(response_event::Event::$enum_type(Default::default())),
            })),
        };
        event_assert!(event, Response($enum_type));
    };
    (Stream, None) => {
        let event = Event {
            event: Some(event::Event::Stream(StreamEvent { event: None })),
        };
        event_assert!(event, Stream(None));
    };
    (Stream, $enum_type:ident) => {
        let event = Event {
            event: Some(event::Event::Stream(StreamEvent {
                event: Some(stream_event::Event::$enum_type(Default::default())),
            })),
        };
        event_assert!(event, Stream($enum_type));
    };
}

#[nativelink_test]
fn get_id_for_event_test() {
    const fn get_expected_value(event: &Event) -> [u8; 2] {
        match &event.event {
            None => [0x00, 0x00],
            Some(event::Event::Request(req)) => {
                match req.event {
                    None => [0x01, 0x00],
                    Some(request_event::Event::GetCapabilitiesRequest(_)) => [0x01, 0x01],
                    Some(request_event::Event::GetActionResultRequest(_)) => [0x01, 0x02],
                    Some(request_event::Event::UpdateActionResultRequest(_)) => [0x01, 0x03],
                    Some(request_event::Event::FindMissingBlobsRequest(_)) => [0x01, 0x04],
                    Some(request_event::Event::BatchReadBlobsRequest(_)) => [0x01, 0x05],
                    Some(request_event::Event::BatchUpdateBlobsRequest(_)) => [0x01, 0x06],
                    Some(request_event::Event::GetTreeRequest(_)) => [0x01, 0x07],
                    Some(request_event::Event::ReadRequest(_)) => [0x01, 0x08],
                    Some(request_event::Event::WriteRequest(())) => [0x01, 0x09],
                    Some(request_event::Event::QueryWriteStatusRequest(_)) => [0x01, 0x0A],
                    Some(request_event::Event::ExecuteRequest(_)) => [0x01, 0x0B],
                    Some(request_event::Event::WaitExecutionRequest(_)) => [0x01, 0x0C],
                    Some(request_event::Event::SchedulerStartExecute(_)) => [0x01, 0x0D],
                    Some(request_event::Event::FetchBlobRequest(_)) => [0x01, 0x0E],
                    Some(request_event::Event::PushBlobRequest(_)) => [0x01, 0x0F],
                    // Don't forget to add new entries to test cases.
                }
            }
            Some(event::Event::Response(res)) => {
                match res.event {
                    None => [0x02, 0x00],
                    Some(response_event::Event::Error(_)) => [0x02, 0x01],
                    Some(response_event::Event::ServerCapabilities(_)) => [0x02, 0x02],
                    Some(response_event::Event::ActionResult(_)) => [0x02, 0x03],
                    Some(response_event::Event::FindMissingBlobsResponse(_)) => [0x02, 0x04],
                    Some(response_event::Event::BatchReadBlobsResponse(_)) => [0x02, 0x05],
                    Some(response_event::Event::BatchUpdateBlobsResponse(_)) => [0x02, 0x06],
                    Some(response_event::Event::WriteResponse(_)) => [0x02, 0x07],
                    Some(response_event::Event::QueryWriteStatusResponse(_)) => [0x02, 0x08],
                    Some(response_event::Event::Empty(())) => [0x02, 0x09],
                    Some(response_event::Event::FetchBlobResponse(_)) => [0x02, 0x0A],
                    Some(response_event::Event::PushBlobResponse(_)) => [0x02, 0x0B],
                    Some(response_event::Event::ActionResourceUsage(_)) => [0x02, 0x0C],
                    // Don't forget to add new entries to test cases.
                }
            }
            Some(event::Event::Stream(stream)) => {
                match stream.event {
                    None => [0x03, 0x00],
                    Some(stream_event::Event::Error(_)) => [0x03, 0x01],
                    Some(stream_event::Event::GetTreeResponse(_)) => [0x03, 0x02],
                    Some(stream_event::Event::DataLength(_)) => [0x03, 0x03],
                    Some(stream_event::Event::WriteRequest(_)) => [0x03, 0x04],
                    Some(stream_event::Event::Operation(_)) => [0x03, 0x05],
                    Some(stream_event::Event::Closed(())) => [0x03, 0x06],
                    // Don't forget to add new entries to test cases.
                }
            }
        }
    }

    let event = Event { event: None };
    event_assert!(event, None);

    test_event!(Request, None);
    test_event!(Request, GetCapabilitiesRequest);
    test_event!(Request, GetActionResultRequest);
    test_event!(Request, UpdateActionResultRequest);
    test_event!(Request, FindMissingBlobsRequest);
    test_event!(Request, BatchReadBlobsRequest);
    test_event!(Request, BatchUpdateBlobsRequest);
    test_event!(Request, GetTreeRequest);
    test_event!(Request, ReadRequest);
    test_event!(Request, WriteRequest);
    test_event!(Request, QueryWriteStatusRequest);
    test_event!(Request, ExecuteRequest);
    test_event!(Request, WaitExecutionRequest);
    test_event!(Request, SchedulerStartExecute);

    test_event!(Response, None);
    test_event!(Response, Error);
    test_event!(Response, ServerCapabilities);
    test_event!(Response, ActionResult);
    test_event!(Response, FindMissingBlobsResponse);
    test_event!(Response, BatchReadBlobsResponse);
    test_event!(Response, BatchUpdateBlobsResponse);
    test_event!(Response, WriteResponse);
    test_event!(Response, QueryWriteStatusResponse);
    test_event!(Response, Empty);
    test_event!(Response, ActionResourceUsage);

    test_event!(Stream, None);
    test_event!(Stream, Error);
    test_event!(Stream, GetTreeResponse);
    test_event!(Stream, DataLength);
    test_event!(Stream, WriteRequest);
    test_event!(Stream, Operation);
    test_event!(Stream, Closed);
}
