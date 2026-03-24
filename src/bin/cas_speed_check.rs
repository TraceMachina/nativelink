use core::time::Duration;
use std::sync::Arc;

use clap::Parser;
use nativelink_error::{Error, ResultExt};
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_client::ContentAddressableStorageClient;
use nativelink_proto::build::bazel::remote::execution::v2::{
    Digest, FindMissingBlobsRequest, digest_function,
};
use nativelink_util::spawn;
use nativelink_util::telemetry::init_tracing;
use nativelink_util::tls_utils::endpoint_from;
use rand::{Rng, RngCore};
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;
use tokio::time::Instant;
use tonic::Request;
use tonic::transport::ClientTlsConfig;
use tracing::info;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[arg(short, long)]
    endpoint: String,

    #[arg(short, long)]
    nativelink_key: Option<String>,
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let args = Args::parse();
    #[expect(
        clippy::disallowed_methods,
        reason = "It's the top-level, so we need the function"
    )]
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            init_tracing(true)?;
            let timings = Arc::new(Mutex::new(Vec::new()));
            let spawns: Vec<_> = (0..200)
                .map(|_| {
                    let local_timings = timings.clone();
                    let local_endpoint = args.endpoint.clone();
                    let local_api_key = args.nativelink_key.clone();
                    spawn!("CAS requester", async move {
                        let tls_config = ClientTlsConfig::new().with_enabled_roots();
                        let endpoint = endpoint_from(&local_endpoint, Some(tls_config))?;
                        let channel = endpoint.connect().await.unwrap();

                        let mut client = ContentAddressableStorageClient::new(channel);

                        for _ in 0..100 {
                            let raw_data: String = rand::rng()
                                .sample_iter::<char, _>(rand::distr::StandardUniform)
                                .take(300)
                                .collect();
                            let hashed = Sha256::digest(raw_data.as_bytes());
                            let rand_hash = hex::encode(hashed);
                            let digest = Digest {
                                hash: rand_hash,
                                size_bytes: i64::from(rand::rng().next_u32()),
                            };

                            let mut request = Request::new(FindMissingBlobsRequest {
                                instance_name: String::new(),
                                blob_digests: vec![digest.clone()],
                                digest_function: digest_function::Value::Sha256.into(),
                            });
                            if let Some(ref api_key) = local_api_key {
                                request
                                    .metadata_mut()
                                    .insert("x-nativelink-api-key", api_key.parse().unwrap());
                            }
                            let start = Instant::now();
                            client
                                .find_missing_blobs(request)
                                .await
                                .err_tip(|| "in find_missing_blobs")?
                                .into_inner();
                            let duration = Instant::now().checked_duration_since(start).unwrap();

                            // info!("response duration={duration:?} res={:?}", res);
                            local_timings.lock().await.push(duration);
                        }
                        Ok::<(), Error>(())
                    })
                })
                .collect();
            for thread in spawns {
                let res = thread.await;
                res.err_tip(|| "with spawn")??;
            }
            let avg = Duration::from_secs_f64({
                let locked = timings.lock().await;
                locked.iter().map(Duration::as_secs_f64).sum::<f64>() / locked.len() as f64
            });
            info!(?avg, "avg");
            Ok::<(), Error>(())
        })?;
    Ok(())
}
