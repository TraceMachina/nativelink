use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as base64_standard;
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use nativelink_error::{Error, make_err};
use sha2::Sha256;
use tonic::Code;
use url::Url;

type HmacSha256 = Hmac<Sha256>;

const SIGNED_VERSION: &str = "2020-12-06";
/// Full container permissions for test convenience: read, add, create,
/// write, delete, list.
const SIGNED_PERMISSIONS: &str = "racwdl";
const SAS_LIFETIME_SECS: i64 = 3600;

/// Builds a container-scoped Service SAS URL signed with Azurite's fixed
/// account key, per the 2020-12-06+ string-to-sign format:
/// <https://learn.microsoft.com/en-us/rest/api/storageservices/create-service-sas>
pub(crate) fn build_container_sas_url(
    blob_endpoint: &str,
    account_name: &str,
    account_key_b64: &str,
    container: &str,
) -> Result<String, Error> {
    let expiry = (Utc::now() + Duration::seconds(SAS_LIFETIME_SECS))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    let canonicalized_resource = format!("/blob/{account_name}/{container}");

    // Field order and the blank fields are both load bearing, the
    // signature covers this exact newline joined layout.
    let string_to_sign = [
        SIGNED_PERMISSIONS,
        "", // signedStart
        &expiry,
        &canonicalized_resource,
        "", // signedIdentifier
        "", // signedIP
        "", // signedProtocolsee doc comment above
        SIGNED_VERSION,
        "c", // signedResource: container
        "",  // signedSnapshotTime
        "",  // signedEncryptionScope
        "",
        "",
        "",
        "",
        "", // rscc, rscd, rsce, rscl, rsct
    ]
    .join("\n");

    let key_bytes = base64_standard
        .decode(account_key_b64)
        .map_err(|e| make_err!(Code::Internal, "Decoding Azurite account key: {e}"))?;

    let mut mac = HmacSha256::new_from_slice(&key_bytes)
        .map_err(|e| make_err!(Code::Internal, "Building HMAC for SAS signing: {e}"))?;
    mac.update(string_to_sign.as_bytes());
    let signature = base64_standard.encode(mac.finalize().into_bytes());

    let mut url = Url::parse(blob_endpoint)
        .map_err(|e| make_err!(Code::Internal, "Invalid Azurite blob endpoint: {e}"))?;
    url.path_segments_mut()
        .map_err(|()| make_err!(Code::Internal, "Azurite blob endpoint is not a base URL"))?
        .pop_if_empty()
        .push(account_name)
        .push(container);

    url.query_pairs_mut()
        .append_pair("sv", SIGNED_VERSION)
        .append_pair("sr", "c")
        .append_pair("sp", SIGNED_PERMISSIONS)
        .append_pair("se", &expiry)
        .append_pair("sig", &signature);

    Ok(url.into())
}

/// Builds an Account SAS query string (not a full URL — meant to be appended
/// to a service, or container level endpoint) authorizing container
/// creation. Unlike a container Service SAS, an Account SAS can authorize
/// operations on resources that don't exist yet, which is what bootstrapping
/// a fresh container needs.
///
/// Scoped narrowly to just "create" (`sp=c`) and container level resources
/// (`srt=c`) — this is a one shot bootstrap credential, not a general purpose
/// one, so it shouldn't grant more than it needs.
pub(crate) fn build_account_sas_query(
    account_name: &str,
    account_key_b64: &str,
) -> Result<String, Error> {
    const SIGNED_PERMISSIONS: &str = "c"; // create
    const SIGNED_SERVICE: &str = "b"; // blob
    const SIGNED_RESOURCE_TYPE: &str = "c"; // container

    let expiry = (Utc::now() + Duration::seconds(SAS_LIFETIME_SECS))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    // Trailing "\n" after signedEncryptionScope is part of the spec, not a
    // typo. Building this via `.join("\n")` alone would silently drop it.
    let string_to_sign = format!(
        "{account_name}\n{SIGNED_PERMISSIONS}\n{SIGNED_SERVICE}\n{SIGNED_RESOURCE_TYPE}\n\n{expiry}\n\n\n{SIGNED_VERSION}\n\n"
    );

    let key_bytes = base64_standard
        .decode(account_key_b64)
        .map_err(|e| make_err!(Code::Internal, "Decoding Azurite account key: {e}"))?;
    let mut mac = HmacSha256::new_from_slice(&key_bytes)
        .map_err(|e| make_err!(Code::Internal, "Building HMAC for account SAS signing: {e}"))?;
    mac.update(string_to_sign.as_bytes());
    let signature = base64_standard.encode(mac.finalize().into_bytes());

    // url::form_urlencoded gives us correct percent encoding without
    // needing a full Url just to build a query fragment.
    let query: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("sv", SIGNED_VERSION)
        .append_pair("ss", SIGNED_SERVICE)
        .append_pair("srt", SIGNED_RESOURCE_TYPE)
        .append_pair("sp", SIGNED_PERMISSIONS)
        .append_pair("se", &expiry)
        .append_pair("sig", &signature)
        .finish();

    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::azurite_runner::{ACCOUNT_KEY, ACCOUNT_NAME};

    #[test]
    #[ignore = "Manual verification tool: requires a live Azurite instance with a hardcoded port, not runnable unattended in CI"]
    #[allow(clippy::print_stdout)]
    fn print_a_sas_url_for_manual_curl_testing() {
        let url = build_container_sas_url(
            "http://127.0.0.1:56117",
            ACCOUNT_NAME,
            ACCOUNT_KEY,
            "test-container-2",
        )
        .unwrap();
        println!("{url}&restype=container&comp=list");
    }
}
