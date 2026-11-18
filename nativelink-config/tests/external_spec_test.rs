// Copyright 2026 Trace Machina, Inc. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use nativelink_config::stores::StoreSpec;
use pretty_assertions::assert_eq;

#[test]
fn external_spec_parses_provider_and_config() {
    let spec: StoreSpec = serde_json5::from_str(
        r#"{
            "external": {
                "provider": "multi_cloud",
                "config": {
                    "primary": {"name": "aws-us-east-1"},
                    "secondaries": [{"name": "oci-phoenix"}],
                }
            }
        }"#,
    )
    .expect("external spec must parse");
    let StoreSpec::External(external) = spec else {
        panic!("expected StoreSpec::External, got {spec:?}");
    };
    assert_eq!(external.provider, "multi_cloud");
    assert_eq!(
        external.config["primary"]["name"],
        serde_json::Value::String("aws-us-east-1".to_string())
    );
    assert_eq!(
        external.config["secondaries"][0]["name"],
        serde_json::Value::String("oci-phoenix".to_string())
    );
}

#[test]
fn external_spec_config_defaults_to_empty_object() {
    let spec: StoreSpec = serde_json5::from_str(r#"{"external": {"provider": "multi_cloud"}}"#)
        .expect("external spec without config must parse");
    let StoreSpec::External(external) = spec else {
        panic!("expected StoreSpec::External, got {spec:?}");
    };
    assert_eq!(
        external.config,
        serde_json::Value::Object(serde_json::Map::new())
    );
}

#[test]
fn external_spec_rejects_unknown_fields() {
    let result: Result<StoreSpec, _> =
        serde_json5::from_str(r#"{"external": {"provider": "multi_cloud", "providr_typo": true}}"#);
    result.expect_err("unknown fields must be rejected");
}

#[test]
fn external_spec_requires_provider() {
    let result: Result<StoreSpec, _> = serde_json5::from_str(r#"{"external": {}}"#);
    result.expect_err("a provider name is required");
}
