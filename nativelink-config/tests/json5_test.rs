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

use std::fs;
use std::path::Path;

use nativelink_config::cas_server::CasConfig;

/// Examples that are deliberately incomplete and only need to *parse*.
///
/// `validate_references` is as strict as startup: a reference into an empty
/// registry, or a dangling `ref_store` inside an unused store, is an error.
/// These two files exist to exercise the parser, not to boot, so they are
/// expected to fail reference validation. If one of them ever starts to pass,
/// it has become a complete deployment config and should be removed from this
/// list so it is held to the same standard as the rest.
const PARSE_ONLY_FRAGMENTS: &[&str] = &[
    // Generated store catalog; carries placeholder `ref_store` names.
    "stores-config.json5",
];

#[test]
fn test_example_parsing() {
    let mut examples_path = Path::new(".")
        .canonicalize()
        .expect("Can canonicalize current dir");

    if examples_path.join("nativelink-config").exists() {
        // inside bazel
        examples_path = examples_path.join("nativelink-config");
    }
    examples_path = examples_path.join("examples");

    let mut found_at_least_one_entry = false;

    for entry in fs::read_dir(&examples_path)
        .unwrap_or_else(|e| panic!("Failed to read from {examples_path:?}: {e}"))
    {
        let config_file = entry.unwrap().path().display().to_string();
        if !config_file.contains(".json5") {
            continue;
        }
        let config = CasConfig::try_from_json5_file(&config_file)
            .unwrap_or_else(|e| panic!("Error while reading {config_file}: {e}"));
        let file_name = Path::new(&config_file)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if PARSE_ONLY_FRAGMENTS.contains(&file_name) {
            assert!(
                config.validate_references().is_err(),
                "{config_file} passes reference validation; remove it from PARSE_ONLY_FRAGMENTS"
            );
        } else {
            config
                .validate_references()
                .unwrap_or_else(|e| panic!("Unresolved reference in {config_file}: {e}"));
        }
        found_at_least_one_entry = true;
    }

    assert!(found_at_least_one_entry);
}
