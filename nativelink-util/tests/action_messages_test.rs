use std::collections::HashMap;
use std::time::SystemTime;

use hex::FromHex;
use nativelink_error::{Code, Error, ErrorContext, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionUniqueKey, ActionUniqueQualifier, ExecutionMetadata,
    INTERNAL_ERROR_EXIT_CODE, to_execute_response,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::precondition_failure;
use prost::Message as _;

fn make_key() -> ActionUniqueKey {
    ActionUniqueKey {
        instance_name: String::from("main"),
        digest_function: DigestHasherFunc::Sha256,
        digest: DigestInfo::new(
            <[u8; 32]>::from_hex(
                "38fc5a8a97c1217160b0b658751979b9a1171286381489c8b98c993ec40b1546",
            )
            .unwrap(),
            210,
        ),
    }
}

#[nativelink_test]
fn old_unique_qualifier_cachable_works() {
    let contents = include_str!("data/action_message_cachable_060.json");
    let action_info: ActionInfo = serde_json::from_str(contents).unwrap();
    assert_eq!(
        action_info.unique_qualifier,
        ActionUniqueQualifier::Cacheable(make_key())
    );
}

#[nativelink_test]
fn old_unique_qualifier_uncachable_works() {
    let contents = include_str!("data/action_message_uncachable_060.json");
    let action_info: ActionInfo = serde_json::from_str(contents).unwrap();
    assert_eq!(
        action_info.unique_qualifier,
        ActionUniqueQualifier::Uncacheable(make_key())
    );
}

const MISSING_DIGEST_HEX: &str = "38fc5a8a97c1217160b0b658751979b9a1171286381489c8b98c993ec40b1546";
const MISSING_DIGEST_SIZE: u64 = 210;

fn missing_digest() -> DigestInfo {
    DigestInfo::new(
        <[u8; 32]>::from_hex(MISSING_DIGEST_HEX).unwrap(),
        MISSING_DIGEST_SIZE,
    )
}

fn make_missing_blob_error(digest: &DigestInfo) -> Error {
    make_err!(
        Code::NotFound,
        "Object {} not found in either fast or slow store. \
            If using multiple workers, ensure all workers share the same CAS storage path.",
        digest,
    )
    .with_context(ErrorContext::MissingDigest {
        hash: digest.packed_hash().to_string(),
        size: digest.size_bytes() as i64,
    })
}

fn action_result_with_error(error: Option<Error>) -> ActionResult {
    ActionResult {
        output_files: Vec::new(),
        output_folders: Vec::new(),
        output_directory_symlinks: Vec::new(),
        output_file_symlinks: Vec::new(),
        exit_code: INTERNAL_ERROR_EXIT_CODE,
        stdout_digest: DigestInfo::new([0u8; 32], 0),
        stderr_digest: DigestInfo::new([0u8; 32], 0),
        execution_metadata: ExecutionMetadata {
            worker: String::new(),
            queued_timestamp: SystemTime::UNIX_EPOCH,
            worker_start_timestamp: SystemTime::UNIX_EPOCH,
            worker_completed_timestamp: SystemTime::UNIX_EPOCH,
            input_fetch_start_timestamp: SystemTime::UNIX_EPOCH,
            input_fetch_completed_timestamp: SystemTime::UNIX_EPOCH,
            execution_start_timestamp: SystemTime::UNIX_EPOCH,
            execution_completed_timestamp: SystemTime::UNIX_EPOCH,
            output_upload_start_timestamp: SystemTime::UNIX_EPOCH,
            output_upload_completed_timestamp: SystemTime::UNIX_EPOCH,
        },
        server_logs: HashMap::new(),
        error,
        message: String::new(),
    }
}

fn assert_missing_blob_status(status: &nativelink_proto::google::rpc::Status, digest: &DigestInfo) {
    use tonic::Code as TonicCode;
    assert_eq!(
        status.code,
        TonicCode::FailedPrecondition as i32,
        "missing-blob errors should be surfaced as FAILED_PRECONDITION",
    );
    assert_eq!(status.details.len(), 1, "expected exactly one detail Any");
    assert_eq!(status.details[0].type_url, precondition_failure::TYPE_URL);
    let pf = precondition_failure::PreconditionFailure::decode(&*status.details[0].value)
        .expect("decoding PreconditionFailure must succeed");
    assert_eq!(pf.violations.len(), 1, "expected one MISSING violation");
    assert_eq!(pf.violations[0].r#type, "MISSING");
    assert_eq!(
        pf.violations[0].subject,
        format!("blobs/{}/{}", digest.packed_hash(), digest.size_bytes()),
        "subject must follow REv2 `blobs/<hash>/<size>` format",
    );
    assert!(
        !pf.violations[0].description.is_empty(),
        "description should carry the underlying error message",
    );
}

#[nativelink_test]
fn to_execute_response_surfaces_missing_blob_as_precondition_failure() {
    let digest = missing_digest();
    let action_result = action_result_with_error(Some(make_missing_blob_error(&digest)));
    let resp = to_execute_response(action_result);
    let status = resp.status.expect("status must be set");
    assert_missing_blob_status(&status, &digest);
}

#[nativelink_test]
fn to_execute_response_detects_missing_blob_through_err_tip_wrapping() {
    let digest = missing_digest();
    let wrapped: Error = Err::<(), _>(make_missing_blob_error(&digest))
        .err_tip(|| "While running the action")
        .err_tip(|| "In worker output upload")
        .unwrap_err();
    let action_result = action_result_with_error(Some(wrapped));
    let resp = to_execute_response(action_result);
    let status = resp.status.expect("status must be set");
    assert_missing_blob_status(&status, &digest);
}

#[nativelink_test]
fn to_execute_response_leaves_non_missing_blob_errors_unchanged() {
    let action_result = action_result_with_error(Some(make_err!(
        Code::NotFound,
        "Some other not-found error that doesn't mention CAS",
    )));
    let resp = to_execute_response(action_result);
    let status = resp.status.expect("status must be set");
    assert_eq!(status.code, Code::NotFound as i32);
    assert!(
        status.details.is_empty(),
        "non-missing-blob errors must not carry a PreconditionFailure detail",
    );
}

#[nativelink_test]
fn to_execute_response_does_not_misclassify_error_without_context() {
    // Pre-typed-context behavior matched on substrings ("Object … not
    // found").
    let action_result = action_result_with_error(Some(make_err!(
        Code::NotFound,
        "Object xyz not found in either fast or slow store",
    )));
    let resp = to_execute_response(action_result);
    let status = resp.status.expect("status must be set");
    assert!(
        status.details.is_empty(),
        "errors without ErrorContext::MissingDigest must not produce a PreconditionFailure detail",
    );
}

#[nativelink_test]
fn to_execute_response_emits_default_status_when_no_error() {
    let resp = to_execute_response(action_result_with_error(None));
    let status = resp.status.expect("status must be set");
    assert_eq!(status.code, 0);
    assert!(status.details.is_empty());
    assert!(status.message.is_empty());
}
