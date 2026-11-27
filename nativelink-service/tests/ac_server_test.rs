// Copyright 2024 Trace Machina, Inc. All rights reserved.
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
use std::sync::Arc;

use bytes::BytesMut;
use nativelink_config::cas_server::WithInstanceName;
use nativelink_config::stores::{MemorySpec, StoreSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::action_cache_server::ActionCache;
use nativelink_proto::build::bazel::remote::execution::v2::{
    ActionResult, Digest, GetActionResultRequest, UpdateActionResultRequest, digest_function,
};
use nativelink_service::ac_server::AcServer;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::StoreLike;
use pretty_assertions::assert_eq;
use prost::Message;
use tonic::{Code, Request, Response, Status};

const INSTANCE_NAME: &str = "foo_instance_name";
const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";
const HASH1_SIZE: i64 = 147;

async fn insert_into_store<T: Message>(
    store: Pin<&impl StoreLike>,
    hash: &str,
    action_size: i64,
    action_result: &T,
) -> Result<i64, Box<dyn core::error::Error>> {
    let mut store_data = BytesMut::new();
    action_result.encode(&mut store_data)?;
    let data_len = store_data.len();
    let digest = DigestInfo::try_new(hash, action_size)?;
    store.update_oneshot(digest, store_data.freeze()).await?;
    Ok(data_len.try_into().unwrap())
}

async fn make_store_manager() -> Result<Arc<StoreManager>, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        "main_cas",
        store_factory(
            &StoreSpec::Memory(MemorySpec::default()),
            &store_manager,
            None,
        )
        .await?,
    )?;
    store_manager.add_store(
        "main_ac",
        store_factory(
            &StoreSpec::Memory(MemorySpec::default()),
            &store_manager,
            None,
        )
        .await?,
    )?;
    Ok(store_manager)
}

fn make_ac_server(store_manager: &StoreManager) -> Result<AcServer, Error> {
    AcServer::new(
        &[WithInstanceName {
            instance_name: "foo_instance_name".to_string(),
            config: nativelink_config::cas_server::AcStoreConfig {
                ac_store: "main_ac".to_string(),
                read_only: false,
            },
        }],
        store_manager,
    )
}

async fn get_action_result(
    ac_server: &AcServer,
    hash: &str,
    size: i64,
) -> Result<Response<ActionResult>, Status> {
    ac_server
        .get_action_result(Request::new(GetActionResultRequest {
            instance_name: INSTANCE_NAME.to_string(),
            action_digest: Some(Digest {
                hash: hash.to_string(),
                size_bytes: size,
            }),
            inline_stdout: false,
            inline_stderr: false,
            inline_output_files: vec![],
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await
}

#[nativelink_test]
async fn empty_store() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let ac_server = make_ac_server(&store_manager)?;

    let raw_response = get_action_result(&ac_server, HASH1, 0).await;

    let err = raw_response.unwrap_err();
    assert_eq!(err.code(), Code::NotFound);
    assert_eq!(err.message(), "");

    Ok(())
}

// An unknown instance name, or a request without its required action digest
// or result, is a bad argument. Bazel retries INTERNAL with backoff but fails
// fast on INVALID_ARGUMENT, so the code decides whether a typo'd
// `--remote_instance_name` fails at once.
#[nativelink_test]
async fn bad_arguments_are_invalid_argument() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let ac_server = make_ac_server(&store_manager)?;
    let get_request = |instance_name: &str, action_digest: Option<Digest>| GetActionResultRequest {
        instance_name: instance_name.to_string(),
        action_digest,
        inline_stdout: false,
        inline_stderr: false,
        inline_output_files: vec![],
        digest_function: digest_function::Value::Sha256.into(),
    };
    let some_digest = Some(Digest {
        hash: HASH1.to_string(),
        size_bytes: 0,
    });

    let unknown_instance = ac_server
        .get_action_result(Request::new(get_request(
            "unknown_instance",
            some_digest.clone(),
        )))
        .await
        .unwrap_err();
    assert_eq!(unknown_instance.code(), Code::InvalidArgument);

    let no_digest = ac_server
        .get_action_result(Request::new(get_request(INSTANCE_NAME, None)))
        .await
        .unwrap_err();
    assert_eq!(no_digest.code(), Code::InvalidArgument);

    let no_result = ac_server
        .update_action_result(Request::new(UpdateActionResultRequest {
            instance_name: INSTANCE_NAME.to_string(),
            action_digest: some_digest,
            action_result: None,
            results_cache_policy: None,
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(no_result.code(), Code::InvalidArgument);
    Ok(())
}

#[nativelink_test]
async fn has_single_item() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let ac_server = make_ac_server(&store_manager)?;
    let ac_store = store_manager.get_store("main_ac").unwrap();

    let action_result = ActionResult {
        exit_code: 45,
        ..Default::default()
    };

    insert_into_store(ac_store.as_pin(), HASH1, HASH1_SIZE, &action_result).await?;
    let raw_response = get_action_result(&ac_server, HASH1, HASH1_SIZE).await;

    assert!(!logs_contain(" output_files: ["));

    assert!(
        raw_response.is_ok(),
        "Expected value, got error {raw_response:?}"
    );
    assert_eq!(raw_response.unwrap().into_inner(), action_result);
    Ok(())
}

#[nativelink_test]
async fn single_item_wrong_digest_size() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let ac_server = make_ac_server(&store_manager)?;
    let ac_store = store_manager.get_store("main_ac").unwrap();

    let action_result = ActionResult {
        exit_code: 45,
        ..Default::default()
    };

    insert_into_store(ac_store.as_pin(), HASH1, HASH1_SIZE, &action_result).await?;
    let raw_response = get_action_result(&ac_server, HASH1, HASH1_SIZE - 1).await;

    let err = raw_response.unwrap_err();
    assert_eq!(err.code(), Code::NotFound);
    assert_eq!(err.message(), "");
    Ok(())
}

fn get_encoded_proto_size<T: Message>(proto: &T) -> Result<usize, Box<dyn core::error::Error>> {
    let mut store_data = Vec::new();
    proto.encode(&mut store_data)?;
    Ok(store_data.len())
}

async fn update_action_result(
    ac_server: &AcServer,
    digest: Digest,
    action_result: ActionResult,
) -> Result<Response<ActionResult>, Status> {
    ac_server
        .update_action_result(Request::new(UpdateActionResultRequest {
            instance_name: INSTANCE_NAME.to_string(),
            action_digest: Some(digest),
            action_result: Some(action_result),
            results_cache_policy: None,
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await
}

#[nativelink_test]
async fn one_item_update_test() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let ac_server = make_ac_server(&store_manager)?;
    let ac_store = store_manager.get_store("main_ac").unwrap();

    let action_result = ActionResult {
        exit_code: 45,
        ..Default::default()
    };

    let size_bytes = get_encoded_proto_size(&action_result)?
        .try_into()
        .unwrap_or(i64::MAX);

    let raw_response = update_action_result(
        &ac_server,
        Digest {
            hash: HASH1.to_string(),
            size_bytes,
        },
        action_result.clone(),
    )
    .await;

    assert!(
        raw_response.is_ok(),
        "Expected success, got error {raw_response:?}"
    );
    assert_eq!(raw_response.unwrap().into_inner(), action_result);

    let digest = DigestInfo::try_new(HASH1, size_bytes)?;
    let raw_data = ac_store.get_part_unchunked(digest, 0, None).await?;

    let decoded_action_result = ActionResult::decode(raw_data)?;
    assert_eq!(decoded_action_result, action_result);
    Ok(())
}
