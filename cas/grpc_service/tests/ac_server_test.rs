// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::io::Cursor;

use tonic::{Code, Request, Response, Status};

use prost::Message;
use proto::build::bazel::remote::execution::v2::{
    action_cache_server::ActionCache, ActionResult, Digest,
};

use ac_server::AcServer;
use store::{create_store, Store, StoreType};

const INSTANCE_NAME: &str = "foo";
const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";

async fn insert_into_store<T: Message>(
    store: &dyn Store,
    hash: &str,
    action_result: &T,
) -> Result<i64, Box<dyn std::error::Error>> {
    let mut store_data = Vec::new();
    action_result.encode(&mut store_data)?;
    let digest_size = store_data.len() as i64;
    store
        .update(&hash, store_data.len(), Box::new(Cursor::new(store_data)))
        .await?;
    Ok(digest_size)
}

#[cfg(test)]
mod get_action_results {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    use proto::build::bazel::remote::execution::v2::GetActionResultRequest;

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
            }))
            .await
    }

    #[tokio::test]
    async fn empty_store() -> Result<(), Box<dyn std::error::Error>> {
        let ac_store = create_store(&StoreType::Memory);
        let ac_server = AcServer::new(ac_store.clone(), create_store(&StoreType::Memory));

        let raw_response = get_action_result(&ac_server, HASH1, 0).await;

        let err = raw_response.unwrap_err();
        assert_eq!(err.code(), Code::NotFound);
        assert_eq!(
            err.message(),
            "Hash 0123456789abcdef000000000000000000000000000000000123456789abcdef not found"
        );
        Ok(())
    }

    #[tokio::test]
    async fn has_single_item() -> Result<(), Box<dyn std::error::Error>> {
        let ac_store = create_store(&StoreType::Memory);
        let ac_server = AcServer::new(ac_store.clone(), create_store(&StoreType::Memory));

        let mut action_result = ActionResult::default();
        action_result.exit_code = 45;

        let digest_size = insert_into_store(ac_store.as_ref(), &HASH1, &action_result).await?;
        let raw_response = get_action_result(&ac_server, HASH1, digest_size).await;

        assert!(
            raw_response.is_ok(),
            "Expected value, got error {:?}",
            raw_response
        );
        assert_eq!(raw_response.unwrap().into_inner(), action_result);
        Ok(())
    }

    #[tokio::test]
    async fn single_item_wrong_digest_size() -> Result<(), Box<dyn std::error::Error>> {
        let ac_store = create_store(&StoreType::Memory);
        let ac_server = AcServer::new(ac_store.clone(), create_store(&StoreType::Memory));

        let mut action_result = ActionResult::default();
        action_result.exit_code = 45;

        let digest_size = insert_into_store(ac_store.as_ref(), &HASH1, &action_result).await?;
        assert!(digest_size > 1, "Digest was too small");
        let raw_response = get_action_result(&ac_server, HASH1, digest_size - 1).await;

        let err = raw_response.unwrap_err();
        assert_eq!(err.code(), Code::NotFound);
        assert_eq!(err.message(), "Found item, but size does not match");
        Ok(())
    }
}

#[cfg(test)]
mod update_action_result {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    use proto::build::bazel::remote::execution::v2::UpdateActionResultRequest;

    fn get_encoded_proto_size<T: Message>(proto: &T) -> Result<usize, Box<dyn std::error::Error>> {
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
            }))
            .await
    }

    #[tokio::test]
    async fn one_item_update_test() -> Result<(), Box<dyn std::error::Error>> {
        let ac_store = create_store(&StoreType::Memory);
        let ac_server = AcServer::new(ac_store.clone(), create_store(&StoreType::Memory));

        let mut action_result = ActionResult::default();
        action_result.exit_code = 45;

        let size_bytes = get_encoded_proto_size(&action_result)?;

        let raw_response = update_action_result(
            &ac_server,
            Digest {
                hash: HASH1.to_string(),
                size_bytes: size_bytes as i64,
            },
            action_result.clone(),
        )
        .await;

        assert!(
            raw_response.is_ok(),
            "Expected success, got error {:?}",
            raw_response
        );
        assert_eq!(raw_response.unwrap().into_inner(), action_result);

        let mut raw_data = Vec::new();
        ac_store
            .get(&HASH1, size_bytes, &mut Cursor::new(&mut raw_data))
            .await?;

        let decoded_action_result = ActionResult::decode(Cursor::new(&raw_data))?;
        assert_eq!(decoded_action_result, action_result);
        Ok(())
    }
}
