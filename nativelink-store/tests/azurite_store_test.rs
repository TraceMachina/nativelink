use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use nativelink_config::stores::ExperimentalAzureSpec;
use nativelink_error::{Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_store::azure_blob_store::AzureBlobStore;
use nativelink_store::cas_utils::ZERO_BYTE_DIGESTS;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::StoreLike;
use tonic::Code;
use uuid::Uuid;

mod azurite_runner;
use azurite_runner::{AzuriteEmbedded, AzuriteParts, create_container};

const VALID_HASH1: &str = "3031323334353637383961626364656630303030303030303030303030303030";

/// Generates a per test container name so concurrently running tests never
/// collide, mirroring how `mongo_store_test`'s spec generates a unique
/// database name per test rather than sharing one.
fn unique_container_name() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!(
        "nltest-{timestamp}-{}",
        &Uuid::new_v4().simple().to_string()[..8]
    )
}

/// Test helper that manages an Azurite-backed `AzureBlobStore`'s lifecycle.
#[derive(Debug)]
pub(crate) struct TestAzuriteHelper {
    pub store: Arc<AzureBlobStore<fn() -> SystemTime>>,
    // Owning `azurite_local` IS the cleanup: dropping it kills the spawned
    // azurite-blob process. Never read directly outside of that, hence the
    // allow, see `AzuriteParts`'s own doc comment.
    #[allow(dead_code)]
    pub azurite_local: Option<AzuriteParts>,
    pub spec: ExperimentalAzureSpec,
}

impl TestAzuriteHelper {
    /// Creates a test `ExperimentalAzureSpec`, either against a real
    /// locally spawned Azurite instance, or, if
    /// `NATIVELINK_TEST_AZURITE_SAS_URL` is set, against an
    /// already running instance the caller provides e.g. for a
    /// developer who'd rather not pay the per test spawn cost while
    /// iterating locally.
    async fn new_spec() -> Result<(ExperimentalAzureSpec, Option<AzuriteParts>), Error> {
        let mut azurite_local: Option<AzuriteParts> = None;
        let container = unique_container_name();

        let sas_url = if let Ok(url) = std::env::var("NATIVELINK_TEST_AZURITE_SAS_URL") {
            url
        } else {
            let azurite = AzuriteEmbedded::new();
            let process = azurite
                .start()
                .await
                .err_tip(|| "Failed to start azurite-blob")?;

            if let Err(e) = create_container(&process.blob_endpoint, &container).await {
                // Surfaces captured stderr on failure rather than just the
                // reqwest level error, since the underlying cause (e.g. an
                // azurite-blob crash) otherwise only shows up as a
                // generic connection error with no indication why.
                let stderr = process.stderr_snapshot().await;
                return Err(make_err!(
                    Code::Internal,
                    "Failed to create container {container}: {e}. azurite-blob stderr: {stderr:?}"
                ));
            }

            let sas_url = azurite.sas_url(&process, &container)?;

            azurite_local.replace(AzuriteParts {
                process,
                embedded: azurite,
            });
            sas_url
        };

        let spec = ExperimentalAzureSpec {
            container,
            sas_url: Some(sas_url),
            ..Default::default()
        };

        Ok((spec, azurite_local))
    }

    // Split out like `mongo_store_test`'s equivalent: separate from `new_spec`
    // because some tests want to edit the spec after getting a live process
    // but before constructing the store.
    async fn new_with_spec_and_process(
        spec: ExperimentalAzureSpec,
        azurite_local: Option<AzuriteParts>,
    ) -> Result<Self, Error> {
        let now_fn: fn() -> SystemTime = SystemTime::now;
        let store = AzureBlobStore::new(&spec, now_fn).await?;
        Ok(Self {
            store,
            azurite_local,
            spec,
        })
    }

    async fn new() -> Result<Self, Error> {
        let (spec, azurite_local) = Self::new_spec().await?;
        Self::new_with_spec_and_process(spec, azurite_local).await
    }
}

impl Drop for TestAzuriteHelper {
    fn drop(&mut self) {
        // No cleanup beyond what dropping `azurite_local` already does
        // (killing the process), the container itself is left in place
        // for inspection, same as Mongo's helper leaves its database.
        eprintln!(
            "Test container retained for inspection: {}",
            self.spec.container
        );
    }
}

#[nativelink_test]
async fn upload_and_get_data() -> Result<(), Error> {
    let helper = TestAzuriteHelper::new().await?;

    let data = Bytes::from_static(b"14");
    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;

    helper.store.update_oneshot(digest, data.clone()).await?;

    let result = helper.store.has(digest).await?;
    assert!(
        result.is_some(),
        "Expected azure store to have hash: {VALID_HASH1}",
    );

    let result = helper
        .store
        .get_part_unchunked(digest, 0, Some(data.len() as u64))
        .await?;

    assert_eq!(result, data, "Expected azure store to have updated value");

    Ok(())
}

#[nativelink_test]
async fn upload_empty_data() -> Result<(), Error> {
    let data = Bytes::from_static(b"");
    let digest = ZERO_BYTE_DIGESTS[0];
    let helper = TestAzuriteHelper::new().await?;

    helper.store.update_oneshot(digest, data).await?;

    let result = helper.store.has(digest).await?;
    assert!(
        result.is_some(),
        "Expected azure store to have zero-byte hash",
    );

    Ok(())
}

#[nativelink_test]
async fn zero_len_items_exist_check() -> Result<(), Error> {
    let digest = DigestInfo::try_new(VALID_HASH1, 0)?;
    let helper = TestAzuriteHelper::new().await?;

    let result = helper.store.get_part_unchunked(digest, 0, None).await;
    let err = result.unwrap_err();
    assert_eq!(err.code, Code::NotFound, "{err:?}");

    Ok(())
}
