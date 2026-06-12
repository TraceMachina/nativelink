use std::path::Path;

use nativelink_config::cas_server::CasConfig;
use nativelink_error::{Code, Error};

#[test]
fn test_duplicate_servers() {
    let mut config_path = Path::new(".")
        .canonicalize()
        .expect("Can canonicalize current dir");

    if config_path.join("nativelink-config").exists() {
        // inside bazel
        config_path = config_path.join("nativelink-config");
    }
    config_path = config_path.join("tests").join("duplicate_servers.json5");

    let err =
        CasConfig::try_from_json5_file(config_path.as_os_str().to_str().unwrap()).unwrap_err();
    assert_eq!(
        err,
        Error::new(
            Code::InvalidArgument,
            "CAS and AC use the same store 'MAIN_STORE' in the config".into()
        )
    );
}
