// Copyright 2023 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use nativelink_config::cas_server::CasConfig;
use nativelink_config::stores::StoreConfig;

const EMBEDDED_JSON: &str = r#"
{
  "stores": {
    "AC_MAIN_STORE": {
      "filesystem": {
        "content_path": "",
        "temp_path": "",
        "eviction_policy": {
          "max_bytes": "1Gb",
          "evict_bytes": "250MiB",
        },
        "read_buffer_size": "1KiB",
        "block_size": "4KiB"
      }
    },
  },
  "servers": [
    {
      "name": "public",
      "listener": {
        "http": {
          "socket_address": "0.0.0.0:50051"
        }
      },
    },
  ],
}

"#;

fn get_cas_config(config: String) -> Result<CasConfig, Box<dyn std::error::Error>> {
    Ok(serde_json5::from_str(&config)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cas_config_correct() {
        let config =
            get_cas_config(EMBEDDED_JSON.to_string()).expect("the given json cannot be parsed");
        let ac_main_store = config
            .stores
            .get("AC_MAIN_STORE")
            .expect("the given json does not contain the field 'AC_MAIN_STORE'");
        if let StoreConfig::filesystem(ac_main_store) = ac_main_store {
            let eviction_policy = ac_main_store
                .eviction_policy
                .clone()
                .expect("eviction_policy should not be none");
            assert_eq!(eviction_policy.max_bytes, 1000000000);
            assert_eq!(eviction_policy.evict_bytes, 262144000);
            assert_eq!(ac_main_store.block_size, 4096);
            assert_eq!(ac_main_store.read_buffer_size, 1024);
        } else {
            panic!("It should be of type StoreConfig::filesystem");
        }
    }
}
