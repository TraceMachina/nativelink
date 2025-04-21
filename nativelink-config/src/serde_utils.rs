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

use core::marker::PhantomData;
use std::borrow::Cow;
use std::fmt;

use byte_unit::Byte;
use humantime::parse_duration;
use serde::de::Visitor;
use serde::{Deserialize, Deserializer, de};

/// Helper for serde macro so you can use shellexpand variables in the json configuration
/// files when the number is a numeric type.
pub fn convert_numeric_with_shellexpand<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<i64>,
    <T as TryFrom<i64>>::Error: fmt::Display,
{
    struct NumericVisitor<T: TryFrom<i64>>(PhantomData<T>);

    impl<T> Visitor<'_> for NumericVisitor<T>
    where
        T: TryFrom<i64>,
        <T as TryFrom<i64>>::Error: fmt::Display,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an integer or a plain number string")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            T::try_from(v).map_err(de::Error::custom)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            let v_i64 = i64::try_from(v).map_err(de::Error::custom)?;
            T::try_from(v_i64).map_err(de::Error::custom)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            let expanded = shellexpand::env(v).map_err(de::Error::custom)?;
            let s = expanded.as_ref().trim();
            let parsed = s.parse::<i64>().map_err(de::Error::custom)?;
            T::try_from(parsed).map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(NumericVisitor::<T>(PhantomData))
}

/// Same as `convert_numeric_with_shellexpand`, but supports `Option<T>`.
pub fn convert_optional_numeric_with_shellexpand<'de, D, T>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<i64>,
    <T as TryFrom<i64>>::Error: fmt::Display,
{
    struct OptionalNumericVisitor<T: TryFrom<i64>>(PhantomData<T>);

    impl<'de, T> Visitor<'de> for OptionalNumericVisitor<T>
    where
        T: TryFrom<i64>,
        <T as TryFrom<i64>>::Error: fmt::Display,
    {
        type Value = Option<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an optional integer or a plain number string")
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2: Deserializer<'de>>(
            self,
            deserializer: D2,
        ) -> Result<Self::Value, D2::Error> {
            deserializer.deserialize_any(self)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            T::try_from(v).map(Some).map_err(de::Error::custom)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            let v_i64 = i64::try_from(v).map_err(de::Error::custom)?;
            T::try_from(v_i64).map(Some).map_err(de::Error::custom)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            if v.is_empty() {
                return Err(de::Error::custom("empty string is not a valid number"));
            }
            if v.trim().is_empty() {
                return Ok(None);
            }
            let expanded = shellexpand::env(v).map_err(de::Error::custom)?;
            let s = expanded.as_ref().trim();
            let parsed = s.parse::<i64>().map_err(de::Error::custom)?;
            T::try_from(parsed).map(Some).map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_option(OptionalNumericVisitor::<T>(PhantomData))
}

/// Helper for serde macro so you can use shellexpand variables in the json
/// configuration files when the input is a string.
///
/// Handles YAML/JSON values according to the YAML 1.2 specification:
/// - Empty string (`""`) remains an empty string
/// - `null` becomes `None`
/// - Missing field becomes `None`
/// - Whitespace is preserved
pub fn convert_string_with_shellexpand<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    Ok((*(shellexpand::env(&value).map_err(de::Error::custom)?)).to_string())
}

/// Same as `convert_string_with_shellexpand`, but supports `Vec<String>`.
pub fn convert_vec_string_with_shellexpand<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    let vec = Vec::<String>::deserialize(deserializer)?;
    vec.into_iter()
        .map(|s| {
            shellexpand::env(&s)
                .map_err(de::Error::custom)
                .map(Cow::into_owned)
        })
        .collect()
}

/// Same as `convert_string_with_shellexpand`, but supports `Option<String>`.
pub fn convert_optional_string_with_shellexpand<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    match value {
        Some(v) if v.is_empty() => Ok(Some(String::new())), // Keep empty string as empty string
        Some(v) => Ok(Some(
            (*(shellexpand::env(&v).map_err(de::Error::custom)?)).to_string(),
        )),
        None => Ok(None), // Handle both null and field not present
    }
}

pub fn convert_data_size_with_shellexpand<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<u128>,
    <T as TryFrom<u128>>::Error: fmt::Display,
{
    struct DataSizeVisitor<T: TryFrom<u128>>(PhantomData<T>);

    impl<T> Visitor<'_> for DataSizeVisitor<T>
    where
        T: TryFrom<u128>,
        <T as TryFrom<u128>>::Error: fmt::Display,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("either a number of bytes as an integer, or a string with a data size format (e.g., \"1GB\", \"500MB\", \"1.5TB\")")
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            T::try_from(u128::from(v)).map_err(de::Error::custom)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            if v < 0 {
                return Err(de::Error::custom("Negative data size is not allowed"));
            }
            T::try_from(v as u128).map_err(de::Error::custom)
        }

        fn visit_u128<E: de::Error>(self, v: u128) -> Result<Self::Value, E> {
            T::try_from(v).map_err(de::Error::custom)
        }

        fn visit_i128<E: de::Error>(self, v: i128) -> Result<Self::Value, E> {
            if v < 0 {
                return Err(de::Error::custom("Negative data size is not allowed"));
            }
            T::try_from(v as u128).map_err(de::Error::custom)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            let expanded = shellexpand::env(v).map_err(de::Error::custom)?;
            let s = expanded.as_ref().trim();
            let byte_size = Byte::parse_str(s, true).map_err(de::Error::custom)?;
            let bytes = byte_size.as_u128();
            T::try_from(bytes).map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(DataSizeVisitor::<T>(PhantomData))
}

pub fn convert_duration_with_shellexpand<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<u64>,
    <T as TryFrom<u64>>::Error: fmt::Display,
{
    struct DurationVisitor<T: TryFrom<u64>>(PhantomData<T>);

    impl<T> Visitor<'_> for DurationVisitor<T>
    where
        T: TryFrom<u64>,
        <T as TryFrom<u64>>::Error: fmt::Display,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("either a number of seconds as an integer, or a string with a duration format (e.g., \"1h2m3s\", \"30m\", \"1d\")")
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            T::try_from(v).map_err(de::Error::custom)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            if v < 0 {
                return Err(de::Error::custom("Negative duration is not allowed"));
            }
            T::try_from(v as u64).map_err(de::Error::custom)
        }

        fn visit_u128<E: de::Error>(self, v: u128) -> Result<Self::Value, E> {
            let v_u64 = u64::try_from(v).map_err(de::Error::custom)?;
            T::try_from(v_u64).map_err(de::Error::custom)
        }

        fn visit_i128<E: de::Error>(self, v: i128) -> Result<Self::Value, E> {
            if v < 0 {
                return Err(de::Error::custom("Negative duration is not allowed"));
            }
            let v_u64 = u64::try_from(v).map_err(de::Error::custom)?;
            T::try_from(v_u64).map_err(de::Error::custom)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            let expanded = shellexpand::env(v).map_err(de::Error::custom)?;
            let expanded = expanded.as_ref().trim();
            let duration = parse_duration(expanded).map_err(de::Error::custom)?;
            let secs = duration.as_secs();
            T::try_from(secs).map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(DurationVisitor::<T>(PhantomData))
}
