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
