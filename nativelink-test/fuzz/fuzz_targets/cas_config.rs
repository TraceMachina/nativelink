#![no_main]

use libfuzzer_sys::fuzz_target;
use nativelink_config::cas_server::CasConfig;

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        let _ = serde_json5::from_str::<CasConfig>(input);
    }
});
