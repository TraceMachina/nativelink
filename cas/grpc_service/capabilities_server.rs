// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use tonic::{Request, Response, Status};

use proto::build::bazel::remote::execution::v2::{
    capabilities_server::Capabilities, capabilities_server::CapabilitiesServer as Server,
    digest_function::Value as DigestFunction, symlink_absolute_path_strategy::Value as SymlinkAbsolutePathStrategy,
    ActionCacheUpdateCapabilities, CacheCapabilities, GetCapabilitiesRequest, ServerCapabilities,
};

use proto::build::bazel::semver::SemVer;

#[derive(Debug, Default)]
pub struct CapabilitiesServer {}

impl CapabilitiesServer {
    pub fn into_service(self) -> Server<CapabilitiesServer> {
        Server::new(self)
    }
}

#[tonic::async_trait]
impl Capabilities for CapabilitiesServer {
    async fn get_capabilities(
        &self,
        _request: Request<GetCapabilitiesRequest>,
    ) -> Result<Response<ServerCapabilities>, Status> {
        let resp = ServerCapabilities {
            cache_capabilities: Some(CacheCapabilities {
                digest_function: vec![DigestFunction::Sha256.into()],
                action_cache_update_capabilities: Some(ActionCacheUpdateCapabilities { update_enabled: true }),
                cache_priority_capabilities: None,
                max_batch_total_size_bytes: 0,
                symlink_absolute_path_strategy: SymlinkAbsolutePathStrategy::Disallowed.into(),
            }),
            execution_capabilities: None,
            deprecated_api_version: None,
            low_api_version: Some(SemVer {
                major: 2,
                minor: 0,
                patch: 0,
                prerelease: "".to_string(),
            }),
            high_api_version: Some(SemVer {
                major: 2,
                minor: 1,
                patch: 0,
                prerelease: "".to_string(),
            }),
        };
        Ok(Response::new(resp))
    }
}
