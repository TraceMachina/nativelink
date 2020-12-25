// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

extern crate cas_server;
extern crate store;

use tonic::Request;

use proto::build::bazel::remote::execution::v2::Digest;

use cas_server::CasServer;
use store::{create_store, StoreType};

#[cfg(test)]
mod find_missing_blobs {
    use super::*;

    use proto::build::bazel::remote::execution::v2::{
        content_addressable_storage_server::ContentAddressableStorage, FindMissingBlobsRequest,
    };

    #[tokio::test]
    async fn empty_store() {
        let cas_server = CasServer::new(create_store(&StoreType::Memory));
        let raw_response = cas_server
            .find_missing_blobs(Request::new(FindMissingBlobsRequest {
                instance_name: "foo".to_string(),
                blob_digests: vec![Digest {
                    hash: "".to_string(),
                    size_bytes: 0,
                }],
            }))
            .await;
        assert!(raw_response.is_ok());
        let response = raw_response.unwrap().into_inner();
        assert_eq!(response.missing_blob_digests.len(), 1);
    }
}
