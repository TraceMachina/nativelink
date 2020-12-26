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

    use std::io::Cursor;

    use tokio::io::Error;

    use proto::build::bazel::remote::execution::v2::{
        content_addressable_storage_server::ContentAddressableStorage, FindMissingBlobsRequest,
    };

    const INSTANCE_NAME: &str = "foo";

    #[tokio::test]
    async fn empty_store() {
        let cas_server = CasServer::new(create_store(&StoreType::Memory));

        const HASH: &str = "123";

        let raw_response = cas_server
            .find_missing_blobs(Request::new(FindMissingBlobsRequest {
                instance_name: INSTANCE_NAME.to_string(),
                blob_digests: vec![Digest {
                    hash: HASH.to_string(),
                    size_bytes: 0,
                }],
            }))
            .await;
        assert!(raw_response.is_ok());
        let response = raw_response.unwrap().into_inner();
        assert_eq!(response.missing_blob_digests.len(), 1);
    }

    #[ignore = "Feature not ready yet"]
    #[tokio::test]
    async fn store_one_item_existence() -> Result<(), Error> {
        let cas_server = CasServer::new(create_store(&StoreType::Memory));

        const HASH: &str = "123";
        const VALUE: &str = "1";

        cas_server
            .store
            .update(&HASH, 1, Box::new(Cursor::new(VALUE)))
            .await?;
        let raw_response = cas_server
            .find_missing_blobs(Request::new(FindMissingBlobsRequest {
                instance_name: INSTANCE_NAME.to_string(),
                blob_digests: vec![Digest {
                    hash: HASH.to_string(),
                    size_bytes: VALUE.len() as i64,
                }],
            }))
            .await;
        assert!(raw_response.is_ok());
        let response = raw_response.unwrap().into_inner();
        assert_eq!(response.missing_blob_digests.len(), 0); // All items should have been found.
        Ok(())
    }
}
