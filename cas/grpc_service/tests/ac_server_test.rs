// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use tonic::{Code, Request};

use proto::build::bazel::remote::execution::v2::Digest;

use ac_server::AcServer;
use proto::build::bazel::remote::execution::v2::action_cache_server::ActionCache;
use store::{create_store, StoreType};

const INSTANCE_NAME: &str = "foo";
const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";

#[cfg(test)]
mod get_action_results {
    use super::*;

    use proto::build::bazel::remote::execution::v2::GetActionResultRequest;

    #[tokio::test]
    #[ignore]
    async fn empty_store() {
        let ac_store = create_store(&StoreType::Memory);
        let cas_store = create_store(&StoreType::Memory);
        let ac_server = AcServer::new(ac_store.clone(), cas_store.clone());

        let raw_response = ac_server
            .get_action_result(Request::new(GetActionResultRequest {
                instance_name: INSTANCE_NAME.to_string(),
                action_digest: Some(Digest {
                    hash: HASH1.to_string(),
                    size_bytes: 0,
                }),
                inline_stdout: false,
                inline_stderr: false,
                inline_output_files: vec![],
            }))
            .await;

        assert!(raw_response.is_err());
        let err = raw_response.unwrap_err();
        assert_eq!(err.code(), Code::NotFound);
        assert_eq!(err.message(), format!("Hash {} not found", HASH1));
    }
}
