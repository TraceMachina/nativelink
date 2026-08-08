use std::fs;
use std::path::{Path, PathBuf};

use nativelink_config::cas_server::CasConfig;

/// Parses every `.json5` config directly under `dir`, returning how many it
/// found. Panics with the offending path on the first parse failure.
fn parse_all_configs_in(dir: &Path) -> usize {
    let mut parsed = 0;
    for entry in
        fs::read_dir(dir).unwrap_or_else(|e| panic!("Failed to read from {}: {e}", dir.display()))
    {
        let config_file = entry.unwrap().path().display().to_string();
        if !config_file.contains(".json5") {
            continue;
        }
        CasConfig::try_from_json5_file(&config_file)
            .unwrap_or_else(|e| panic!("Error while reading {config_file}: {e}"));
        parsed += 1;
    }
    parsed
}

/// The repository root, whether the test runs from the workspace root or from
/// inside the `nativelink-config` package (as it does under bazel).
fn repo_root() -> PathBuf {
    let cwd = Path::new(".")
        .canonicalize()
        .expect("Can canonicalize current dir");
    if cwd.join("nativelink-config").exists() {
        cwd
    } else {
        cwd.parent()
            .expect("nativelink-config has a parent directory")
            .to_path_buf()
    }
}

#[test]
fn test_example_parsing() {
    let examples_path = repo_root().join("nativelink-config").join("examples");
    assert!(
        parse_all_configs_in(&examples_path) > 0,
        "expected at least one example config in {}",
        examples_path.display()
    );
}

/// The `deployment-examples` trees are what operators copy, so a config that
/// does not even parse is worse than a missing one. Nothing used to check them,
/// which let an unbalanced brace ship in `local-storage-cas-zstd.json5`.
#[test]
fn test_deployment_example_parsing() {
    let deployment_examples = repo_root().join("deployment-examples");
    if !deployment_examples.exists() {
        // Not present in the bazel runfiles for this target; the cargo run covers it.
        return;
    }

    let mut parsed = 0;
    for entry in fs::read_dir(&deployment_examples)
        .unwrap_or_else(|e| panic!("Failed to read from {}: {e}", deployment_examples.display()))
    {
        let dir = entry.unwrap().path();
        if dir.is_dir() {
            parsed += parse_all_configs_in(&dir);
        }
    }

    assert!(
        parsed > 0,
        "expected at least one deployment example config under {}",
        deployment_examples.display()
    );
}
