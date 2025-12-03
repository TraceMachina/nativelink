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
        .unwrap_or_else(|e| panic!("Failed to read from {:?}: {}", &examples_path, e))
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
