// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use nativelink_config::serde_utils::{
    convert_data_size_with_shellexpand, convert_duration_with_shellexpand,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;

#[derive(Deserialize)]
struct DurationEntity {
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    duration: usize,
}

#[derive(Deserialize)]
struct DataSizeEntity {
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    data_size: usize,
}

#[test]
fn test_duration_human_readable_deserialize() {
    let example = r#"
            {"duration": "1m 10s"}
        "#;
    let deserialized: DurationEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.duration, 70);
}

#[test]
fn test_duration_usize_deserialize() {
    let example = r#"
            {"duration": 10}
        "#;
    let deserialized: DurationEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.duration, 10);
}

#[test]
fn test_data_size_unit_deserialize() {
    let example = r#"
            {"data_size": "1KiB"}
        "#;
    let deserialized: DataSizeEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.data_size, 1024);
}

#[test]
fn test_data_size_usize_deserialize() {
    let example = r#"
            {"data_size": 10}
        "#;
    let deserialized: DataSizeEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.data_size, 10);
}
