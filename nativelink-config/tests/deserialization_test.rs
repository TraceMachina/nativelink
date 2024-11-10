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
    convert_numeric_with_shellexpand, convert_optional_numeric_with_shellexpand,
    convert_optional_string_with_shellexpand, convert_string_with_shellexpand,
    convert_vec_string_with_shellexpand,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;

const INVALID_ENV_VAR_ERROR: &str =
    "error looking key 'INVALID_ENV_VAR' up: environment variable not found";
const MAP_TYPE_ERROR: &str = "invalid type: map, expected a string containing json data";

const DURATION_HUMAN_READABLE: &str = r#"{"duration": "1m 10s"}"#;
const DURATION_USIZE: &str = r#"{"duration": 10}"#;
const DATA_SIZE_KIB: &str = r#"{"data_size": "1KiB"}"#;
const DATA_SIZE_USIZE: &str = r#"{"data_size": 10}"#;
const STRING_ENV_VAR: &str = r#"{"expanded_string": "$INVALID_ENV_VAR"}"#;
const EMPTY_STRING_ENV_VAR: &str = r#"{"expanded_string": ""}"#;
const NULL_STRING_ENV_VAR: &str = r#"{"expanded_string": null}"#;
const VEC_STRING_EMPTY: &str = r#"{"expanded_strings": []}"#;

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

#[derive(Deserialize)]
struct NumericEntity {
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    value: usize,
}

#[derive(Deserialize)]
struct OptionalNumericEntity {
    #[serde(
        default,
        deserialize_with = "convert_optional_numeric_with_shellexpand"
    )]
    optional_value: Option<usize>,
}

#[derive(Deserialize)]
struct VecStringEntity {
    #[serde(deserialize_with = "convert_vec_string_with_shellexpand")]
    expanded_strings: Vec<String>,
}

#[derive(Deserialize)]
struct OptionalStringEntity {
    #[serde(deserialize_with = "convert_optional_string_with_shellexpand")]
    expanded_string: Option<String>,
}

#[derive(Deserialize)]
struct StringEntity {
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    expanded_string: String,
}

#[test]
fn test_duration_human_readable_deserialize() {
    let example = DURATION_HUMAN_READABLE;
    let deserialized: DurationEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.duration, 70);
}

#[test]
fn test_duration_usize_deserialize() {
    let example = DURATION_USIZE;
    let deserialized: DurationEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.duration, 10);
}

#[test]
fn test_duration_invalid_string() {
    let example = r#"{"duration": {size:10, in:8}}"#;
    let result: Result<DurationEntity, _> = serde_json5::from_str(example);
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.to_string(), MAP_TYPE_ERROR);
    }
}

#[test]
fn test_data_size_unit_deserialize() {
    let example = DATA_SIZE_KIB;
    let deserialized: DataSizeEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.data_size, 1024);
}

#[test]
fn test_data_size_usize_deserialize() {
    let example = DATA_SIZE_USIZE;
    let deserialized: DataSizeEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.data_size, 10);
}

#[test]
fn test_data_size_invalid_string() {
    let example = r#"{"data_size": {size:10, in:8}}"#;
    let result: Result<DataSizeEntity, _> = serde_json5::from_str(example);
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.to_string(), MAP_TYPE_ERROR);
    }
}

#[test]
fn test_numeric_with_shellexpand_integer() {
    let example = r#"{ "value": 42 }"#;
    let deserialized: NumericEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.value, 42);
}

#[test]
fn test_numeric_with_shellexpand_string() {
    let example = r#"{ "value": "100" }"#;
    let deserialized: NumericEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.value, 100);
}

#[test]
fn test_numeric_with_shellexpand_invalid_string() {
    let example = r#"{ "value": {size:10, in:8} }"#;
    let result: Result<NumericEntity, _> = serde_json5::from_str(example);
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.to_string(), MAP_TYPE_ERROR);
    }
}

#[test]
fn test_optional_numeric_with_shellexpand_integer() {
    let example = r#"{ "optional_value": 42 }"#;
    let deserialized: OptionalNumericEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.optional_value, Some(42));
}

#[test]
fn test_optional_numeric_with_shellexpand_string() {
    let example = r#"{ "optional_value": "100" }"#;
    let deserialized: OptionalNumericEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.optional_value, Some(100));
}

#[test]
fn test_optional_numeric_with_shellexpand_empty_string() {
    let example = r#"{"optional_value": ""}"#;
    let deserialized: OptionalNumericEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.optional_value, None);
}

#[test]
fn test_optional_numeric_with_shellexpand_invalid_string() {
    let example = r#"{ "optional_value": {size:10, in:8} }"#;
    let result: Result<OptionalNumericEntity, _> = serde_json5::from_str(example);
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.to_string(), MAP_TYPE_ERROR);
    }
}

#[test]
fn test_convert_string_with_shellexpand_invalid() {
    let example = STRING_ENV_VAR;
    let result: Result<StringEntity, _> = serde_json5::from_str(example);
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.to_string(), INVALID_ENV_VAR_ERROR);
    }
}

#[test]
fn test_convert_string_with_shellexpand_empty() {
    let example = EMPTY_STRING_ENV_VAR;
    let deserialized: StringEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.expanded_string, "");
}

#[test]
fn test_convert_vec_string_with_shellexpand_empty() {
    let example = VEC_STRING_EMPTY;
    let deserialized: VecStringEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.expanded_strings, Vec::<String>::new());
}

#[test]
fn test_convert_vec_string_with_shellexpand_invalid() {
    let example = r#"{"expanded_strings": ["$INVALID_ENV_VAR", "$ANOTHER_MISSING_VAR"]}"#;
    let result: Result<VecStringEntity, _> = serde_json5::from_str(example);
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.to_string(), INVALID_ENV_VAR_ERROR);
    }
}

#[test]
fn test_convert_vec_string_with_shellexpand_invalid_alternate() {
    let example = r#"{"expanded_strings": ["config.json", "$INVALID_ENV_VAR"]}"#;
    let result: Result<VecStringEntity, _> = serde_json5::from_str(example);
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.to_string(), INVALID_ENV_VAR_ERROR);
    }
}

#[test]
fn test_convert_optional_string_with_shellexpand_none() {
    let example = NULL_STRING_ENV_VAR;
    let deserialized: OptionalStringEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.expanded_string, None);
}

#[test]
fn test_convert_optional_string_with_shellexpand_empty() {
    let example = EMPTY_STRING_ENV_VAR;
    let deserialized: OptionalStringEntity = serde_json5::from_str(example).unwrap();
    assert_eq!(deserialized.expanded_string, Some(String::new()));
}

#[test]
fn test_convert_optional_string_with_shellexpand_invalid() {
    let example = STRING_ENV_VAR;
    let result: Result<OptionalStringEntity, _> = serde_json5::from_str(example);
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.to_string(), INVALID_ENV_VAR_ERROR);
    }
}
