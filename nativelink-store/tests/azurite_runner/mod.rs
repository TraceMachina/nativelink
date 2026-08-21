#![allow(dead_code)]

mod process;
mod sas;

use core::time::Duration;
use std::path::{Path, PathBuf};

use nativelink_error::{Error, ResultExt, make_err};
use process::AzuriteProcess;
use tempfile::tempdir;
use tonic::Code;
use tracing::debug;

pub(crate) use sas::build_container_sas_url;

/// Owning `process` IS the cleanup. dropping it kills the spawned
/// `azurite-blob` via `AzuriteProcess`'s `Drop` impl. `embedded` is kept
/// alongside it only so its fields (bind_ip, location) remain inspectable
/// for the life of the test.
#[derive(Debug)]
pub(crate) struct AzuriteParts {
    pub process: AzuriteProcess,
    pub embedded: AzuriteEmbedded,
}

/// Azurite's fixed, publicly documented dev-storage account and key — not a
/// secret. Every Azurite instance anywhere accepts this by default; see
/// https://learn.microsoft.com/en-us/azure/storage/common/storage-connect-azurite
pub(crate) const ACCOUNT_NAME: &str = "devstoreaccount1";
pub(crate) const ACCOUNT_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

#[derive(Debug)]
pub(crate) struct AzuriteEmbedded {
    pub bind_ip: String,
    /// Directory Azurite stores its blob data in. Unlike Mongo's `db_path`,
    /// this has no meaningful reuse-across-runs behavior, each test gets
    /// its own fresh temp dir, so there's no equivalent of Mongo's
    /// download/extract cache to preserve here.
    pub location: PathBuf,
}

impl AzuriteEmbedded {
    pub(crate) fn new() -> Self {
        Self {
            bind_ip: "127.0.0.1".to_string(),
            location: tempdir()
                .expect("Failed to create temporary directory")
                .keep(),
        }
    }

    pub(crate) fn set_bind_ip(mut self, bind_ip: &str) -> Self {
        self.bind_ip = bind_ip.to_string();
        self
    }

    pub(crate) async fn start(&self) -> Result<AzuriteProcess, Error> {
        let binary_path = find_azurite_binary()?;

        let mut process = AzuriteProcess::start(&binary_path, &self.location, &self.bind_ip)
            .await
            .err_tip(|| format!("Starting azurite-blob from {}", binary_path.display()))?;

        wait_until_ready(&mut process).await?;

        Ok(process)
    }

    /// Builds a container-scoped SAS URL against the running Azurite
    /// instance, for use in `ExperimentalAzureSpec.sas_url`. `endpoint` is
    /// left unset in the spec since `sas_url` takes precedence and is used
    /// as is by `AzureBlobStore::build_container_client`.
    pub(crate) fn sas_url(
        &self,
        process: &AzuriteProcess,
        container: &str,
    ) -> Result<String, Error> {
        build_container_sas_url(&process.blob_endpoint, ACCOUNT_NAME, ACCOUNT_KEY, container)
    }
}

/// Resolves the installed `azurite-blob` binary relative to this crate,
/// rather than shelling out through `npx` per test. `npx`'s own package
/// resolution adds meaningfully non trivial overhead even when the package
/// is already cached, negligible for a single manual run, but this test
/// suite spawns Azurite once per test, so that overhead multiplies across
/// the whole suite. Requires `npm ci` to have been run in this directory
/// first; see this crate's `tests/azurite_runner/package.json`.
fn find_azurite_binary() -> Result<PathBuf, Error> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("azurite_runner")
        .join("node_modules")
        .join(".bin")
        .join("azurite-blob");

    if !path.exists() {
        return Err(make_err!(
            Code::NotFound,
            "azurite-blob not found at {}. Run `npm ci` in nativelink-store/tests/azurite_runner first.",
            path.display()
        ));
    }

    Ok(path)
}

/// Polls until the just spawned `azurite-blob` answers on its blob
/// endpoint. Unlike Mongo's readiness check, we don't need a successful
/// authenticated response here, even Azurite's unauthenticated 400 is
/// sufficient proof the server is up and speaking HTTP, since the actual
/// SAS signed requests happen later, once the caller has a real container.
async fn wait_until_ready(process: &mut AzuriteProcess) -> Result<(), Error> {
    drop(rustls::crypto::ring::default_provider().install_default());

    let client = reqwest::Client::new();
    let start = std::time::Instant::now();

    while start.elapsed() < Duration::from_secs(30) {
        if let Some(status) = process.try_wait()? {
            return Err(make_err!(
                Code::Internal,
                "azurite-blob exited during startup with {status}"
            ));
        }

        if client
            .get(format!("{}/{ACCOUNT_NAME}", process.blob_endpoint))
            .send()
            .await
            .is_ok()
        {
            debug!("Connected to azurite-blob at {}", process.blob_endpoint);
            return Ok(());
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Err(make_err!(
        Code::DeadlineExceeded,
        "Timed out waiting for azurite-blob to start"
    ))
}

/// A container scoped SAS can't authorize creating the container it names
/// the signature is only valid for a resource that already exists, so
/// bootstrapping needs a broader Account SAS instead (see `sas.rs`).
///
/// Retries with a short backoff rather than a single attempt: Azurite can
/// accept its first connection (our readiness check) before some of its
/// own internal subsystems (telemetry, request logging) have finished
/// initializing, and a request landing in that narrow window can be
/// rejected or, in one observed case, crash the Azurite process outright.
/// A `CONFLICT` (409) response is treated as success, it means a
/// previous attempt's request actually landed despite an error being
/// reported back to us, which is expected and harmless here since
/// container names are per-test-unique anyway.
pub(crate) async fn create_container(blob_endpoint: &str, container: &str) -> Result<(), Error> {
    drop(rustls::crypto::ring::default_provider().install_default());

    let account_sas = sas::build_account_sas_query(ACCOUNT_NAME, ACCOUNT_KEY)?;
    let url = format!("{blob_endpoint}/{ACCOUNT_NAME}/{container}?restype=container&{account_sas}");
    let client = reqwest::Client::new();

    let mut last_err = None;
    for _ in 0..5 {
        match client.put(&url).body(Vec::new()).send().await {
            Ok(response)
                if response.status().is_success()
                    || response.status() == reqwest::StatusCode::CONFLICT =>
            {
                return Ok(());
            }
            Ok(response) => {
                last_err = Some(make_err!(
                    Code::Internal,
                    "Creating Azurite container {container} failed with {}",
                    response.status()
                ));
            }
            Err(e) => {
                last_err = Some(make_err!(
                    Code::Internal,
                    "Creating Azurite container {container}: {e}"
                ));
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    Err(last_err.unwrap_or_else(|| make_err!(Code::Internal, "create_container exhausted retries")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Manual verification tool: requires a live Azurite instance with a hardcoded port, not runnable unattended in CI"]
    async fn manually_verify_create_container() {
        create_container("http://127.0.0.1:56117", "test-container-2")
            .await
            .unwrap();
    }
}
