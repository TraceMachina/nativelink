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

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use nativelink_config::cas_server::CasConfig;
use tracing_test::traced_test;

/// Directories, relative to the repository root, whose configs are checked.
const CONFIG_DIRS: &[&str] = &[
    "nativelink-config/examples",
    "deployment-examples",
    "integration_tests",
    "kubernetes",
];

/// Configs that are deliberately incomplete and only need to *parse*.
///
/// `validate_references` is as strict as startup: a reference into an empty
/// registry, or a dangling `ref_store` inside an unused store, is an error.
/// This file exists to exercise the parser, not to boot, so it is expected to
/// fail reference validation. If a file listed here ever starts to pass, it
/// has become a complete deployment config and should be removed from this
/// list so it is held to the same standard as the rest.
const PARSE_ONLY_FRAGMENTS: &[&str] = &[
    // Generated store catalog; carries placeholder `ref_store` names.
    "stores-config.json5",
];

/// The one config allowed to use the deprecated map format. It exists to show
/// that shape to people migrating, so it must keep using it.
const LEGACY_CONFIG: &str = "legacy_service_config.json5";

/// The first line of the warning `backcompat` logs for a deprecated map.
const DEPRECATION_NOTICE: &str = "WARNING: Using deprecated map format";

/// The repository root: Bazel runs tests from the runfiles root, Cargo from
/// the crate directory.
fn repo_root() -> PathBuf {
    let cwd = Path::new(".")
        .canonicalize()
        .expect("Can canonicalize current dir");
    if cwd.join("nativelink-config").exists() {
        cwd
    } else {
        cwd.parent().expect("crate dir has a parent").to_path_buf()
    }
}

/// Every `.json5` file under `dir`, recursively, in a stable order.
fn json5_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("Failed to read from {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(json5_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "json5") {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// Every config in [`CONFIG_DIRS`] must parse and pass reference validation
/// (except [`PARSE_ONLY_FRAGMENTS`]), and must not use the deprecated map
/// format (except [`LEGACY_CONFIG`], which must).
#[test]
#[traced_test]
fn test_all_configs() {
    let root = repo_root();
    let configs: Vec<PathBuf> = CONFIG_DIRS
        .iter()
        .flat_map(|dir| json5_files(&root.join(dir)))
        .collect();
    assert!(
        configs.iter().any(|path| path.ends_with(LEGACY_CONFIG)),
        "{LEGACY_CONFIG} not found under {}",
        root.display()
    );

    // Logs build up over the test, so count the notices each file adds.
    let notices_so_far = || {
        let count = Cell::new(0);
        logs_assert(|lines: &[&str]| {
            count.set(
                lines
                    .iter()
                    .filter(|line| line.contains(DEPRECATION_NOTICE))
                    .count(),
            );
            Ok(())
        });
        count.get()
    };

    let mut seen = 0;
    for config_path in &configs {
        let name = config_path
            .strip_prefix(&root)
            .unwrap_or(config_path)
            .display();
        let config = CasConfig::try_from_json5_file(&config_path.display().to_string())
            .unwrap_or_else(|e| panic!("Error while reading {name}: {e}"));

        let file_name = config_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if PARSE_ONLY_FRAGMENTS.contains(&file_name) {
            assert!(
                config.validate_references().is_err(),
                "{name} passes reference validation; remove it from PARSE_ONLY_FRAGMENTS"
            );
        } else {
            config
                .validate_references()
                .unwrap_or_else(|e| panic!("Unresolved reference in {name}: {e}"));
        }

        let total = notices_so_far();
        let added = total - seen;
        seen = total;
        if file_name == LEGACY_CONFIG {
            assert!(
                added > 0,
                "{name} no longer uses the deprecated map format it exists to show"
            );
        } else {
            assert_eq!(
                added, 0,
                "{name} uses a deprecated map format; convert it to the list form"
            );
        }
    }
}
