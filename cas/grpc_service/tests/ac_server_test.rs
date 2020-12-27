// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::io::Cursor;

use tokio::io::Error;
use tonic::{Code, Request, Response, Status};

use prost::Message;
use proto::build::bazel::remote::execution::v2::Digest;

use ac_server::AcServer;
use proto::build::bazel::remote::execution::v2::{action_cache_server::ActionCache, ActionResult};
use store::{create_store, Store, StoreType};

const INSTANCE_NAME: &str = "foo";
const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";

#[cfg(test)]
mod get_action_results {
    use super::*;

    use proto::build::bazel::remote::execution::v2::GetActionResultRequest;

    async fn insert_into_store<T: Message>(
        store: &dyn Store,
        hash: &str,
        action_result: &T,
    ) -> Result<i64, Error> {
        let mut store_data = Vec::new();
        action_result.encode(&mut store_data)?;
        let digest_size = store_data.len() as i64;
        store
            .update(&hash, store_data.len(), Box::new(Cursor::new(store_data)))
            .await?;
        Ok(digest_size)
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
            }))
            .await
    }

    #[tokio::test]
    async fn empty_store() -> Result<(), Error> {
        let ac_store = create_store(&StoreType::Memory);
        let ac_server = AcServer::new(ac_store.clone(), create_store(&StoreType::Memory));

        let raw_response = get_action_result(&ac_server, HASH1, 0).await;

        let err = raw_response.unwrap_err();
        assert_eq!(err.code(), Code::NotFound);
        assert_eq!(err.message(), "");
        Ok(())
    }

    #[tokio::test]
    async fn has_single_item() -> Result<(), Error> {
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
    async fn single_item_wrong_digest_size() -> Result<(), Error> {
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
