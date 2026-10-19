// Copyright 2026 The NativeLink Authors. All rights reserved.
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
        CasConfig::try_from_json5_file(&config_file)
            .unwrap_or_else(|e| panic!("Error while reading {config_file}: {e}"));
        found_at_least_one_entry = true;
    }

    assert!(found_at_least_one_entry);
}
