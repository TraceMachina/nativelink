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

use error::{error_if, make_err, Code, Error, ResultExt};
use store::Store;

pub struct AcServer {
    ac_store: Arc<dyn Store>,
    _cas_store: Arc<dyn Store>,
}

impl AcServer {
    pub fn new(ac_store: Arc<dyn Store>, cas_store: Arc<dyn Store>) -> Self {
        AcServer {
            ac_store: ac_store,
            _cas_store: cas_store,
        }
    }

    pub fn into_service(self) -> Server<AcServer> {
        Server::new(self)
    }

    async fn inner_get_action_result(
        &self,
        grpc_request: Request<GetActionResultRequest>,
    ) -> Result<Response<ActionResult>, Error> {
        let get_action_request = grpc_request.into_inner();

        // TODO(blaise.bruer) This needs to be fixed. It is using wrong macro.
        // We also should write a test for these errors.
        let digest = get_action_request
            .action_digest
            .err_tip(|| "Action digest was not set in message")?;
        let size_bytes = usize::try_from(digest.size_bytes)
            .err_tip(|| "Digest size_bytes was not convertable to usize")?;

        // TODO(allada) There is a security risk here of someone taking all the memory on the instance.
        let mut store_data = Vec::with_capacity(size_bytes);
        let mut cursor = Cursor::new(&mut store_data);
        self.ac_store
            .get(&digest.hash, size_bytes, &mut cursor)
            .await?;

        let action_result =
            ActionResult::decode(Cursor::new(&store_data)).err_tip_with_code(|e| {
                (
                    Code::NotFound,
                    format!("Stored value appears to be corrupt: {}", e),
                )
            })?;

        if store_data.len() != size_bytes {
            return Err(make_err!(
                Code::NotFound,
                "Found item, but size does not match"
            ));
        }
        Ok(Response::new(action_result))
    }

    async fn inner_update_action_result(
        &self,
        grpc_request: Request<UpdateActionResultRequest>,
    ) -> Result<Response<ActionResult>, Error> {
        let update_action_request = grpc_request.into_inner();

        // TODO(blaise.bruer) This needs to be fixed. It is using wrong macro.
        // We also should write a test for these errors.
        let digest = update_action_request
            .action_digest
            .err_tip(|| "Action digest was not set in message")?;

        let size_bytes = usize::try_from(digest.size_bytes)
            .err_tip(|| "Digest size_bytes was not convertable to usize")?;

        let action_result = update_action_request
            .action_result
            .err_tip(|| "Action result was not set in message")?;

        // TODO(allada) There is a security risk here of someone taking all the memory on the instance.
        let mut store_data = Vec::with_capacity(size_bytes);
        action_result
            .encode(&mut store_data)
            .err_tip(|| "Provided ActionResult could not be serialized")?;
        error_if!(
            store_data.len() != size_bytes,
            "Digest size does not match. Actual: {} Expected: {} ",
            store_data.len(),
            size_bytes
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

#[tonic::async_trait]
impl ActionCache for AcServer {
    async fn get_action_result(
        &self,
        grpc_request: Request<GetActionResultRequest>,
    ) -> Result<Response<ActionResult>, Status> {
        println!("get_action_result Req: {:?}", grpc_request);
        let resp = self.inner_get_action_result(grpc_request).await;
        println!("get_action_result Resp: {:?}", resp);
        return resp.map_err(|e| e.into());
    }

    async fn update_action_result(
        &self,
        grpc_request: Request<UpdateActionResultRequest>,
    ) -> Result<Response<ActionResult>, Status> {
        println!("update_action_result Req: {:?}", grpc_request);
        let resp = self.inner_update_action_result(grpc_request).await;
        println!("update_action_result Resp: {:?}", resp);
        return resp.map_err(|e| e.into());
    }
}
