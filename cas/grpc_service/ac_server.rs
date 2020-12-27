// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::convert::TryFrom;
use std::io::Cursor;
use std::sync::Arc;

use prost::Message;
use tonic::{Request, Response, Status};

use proto::build::bazel::remote::execution::v2::{
    action_cache_server::ActionCache, action_cache_server::ActionCacheServer as Server,
    ActionResult, GetActionResultRequest, UpdateActionResultRequest,
};

use macros::error_if;
use store::Store;

#[derive(Debug)]
pub struct AcServer {
    ac_store: Arc<dyn Store>,
    cas_store: Arc<dyn Store>,
}

impl AcServer {
    pub fn new(ac_store: Arc<dyn Store>, cas_store: Arc<dyn Store>) -> Self {
        AcServer {
            ac_store: ac_store,
            cas_store: cas_store,
        }
    }

    pub fn into_service(self) -> Server<AcServer> {
        Server::new(self)
    }
}

#[tonic::async_trait]
impl ActionCache for AcServer {
    async fn get_action_result(
        &self,
        grpc_request: Request<GetActionResultRequest>,
    ) -> Result<Response<ActionResult>, Status> {
        let get_action_request = grpc_request.into_inner();

        // TODO(blaise.bruer) This needs to be fixed. It is using wrong macro.
        // We also should write a test for these errors.
        let digest = get_action_request
            .action_digest
            .ok_or(Status::invalid_argument(
                "Action digest was not set in message",
            ))?;
        let size_bytes = usize::try_from(digest.size_bytes).or(Err(Status::invalid_argument(
            "Digest size_bytes was not convertable to usize",
        )))?;

        // TODO(allada) There is a security risk here of someone taking all the memory on the instance.
        let mut store_data = Vec::with_capacity(size_bytes);
        self.ac_store
            .get(&digest.hash, size_bytes, &mut Cursor::new(&mut store_data))
            .await
            .or(Err(Status::not_found("")))?;

        let action_result = ActionResult::decode(Cursor::new(&store_data)).or(Err(
            Status::not_found("Stored value appears to be corrupt."),
        ))?;

        error_if!(
            store_data.len() != size_bytes,
            Status::not_found("Found item, but size does not match")
        );
        Ok(Response::new(action_result))
    }

    async fn update_action_result(
        &self,
        grpc_request: Request<UpdateActionResultRequest>,
    ) -> Result<Response<ActionResult>, Status> {
        let update_action_request = grpc_request.into_inner();

        // TODO(blaise.bruer) This needs to be fixed. It is using wrong macro.
        // We also should write a test for these errors.
        let digest = update_action_request
            .action_digest
            .ok_or(Status::invalid_argument(
                "Action digest was not set in message",
            ))?;

        let size_bytes = usize::try_from(digest.size_bytes).or(Err(Status::invalid_argument(
            "Digest size_bytes was not convertable to usize",
        )))?;

        let action_result = update_action_request
            .action_result
            .ok_or(Status::invalid_argument(
                "Action result was not set in message",
            ))?;

        // TODO(allada) There is a security risk here of someone taking all the memory on the instance.
        let mut store_data = Vec::with_capacity(size_bytes);
        action_result
            .encode(&mut store_data)
            .or(Err(Status::invalid_argument(
                "Provided ActionResult could not be serialized",
            )))?;
        error_if!(
            store_data.len() != size_bytes,
            Status::invalid_argument("Provided digest size does not match serialized size")
        );
        self.ac_store
            .update(
                &digest.hash,
                store_data.len(),
                Box::new(Cursor::new(store_data)),
            )
            .await?;
        Ok(Response::new(action_result))
    }
}
