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

use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use byte_unit::Byte;
use humantime::parse_duration;
use serde::{de, Deserialize, Deserializer};

/// Helper for serde macro so you can use shellexpand variables in the json configuration
/// files when the number is a numeric type.
pub fn convert_numeric_with_shellexpand<'de, D, T, E>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    E: fmt::Display,
    T: TryFrom<i64> + FromStr<Err = E>,
    <T as TryFrom<i64>>::Error: fmt::Display,
{
    // define a visitor that deserializes
    // `ActualData` encoded as json within a string
    struct USizeVisitor<T: TryFrom<i64>>(PhantomData<T>);

    impl<'de, T, FromStrErr> de::Visitor<'de> for USizeVisitor<T>
    where
        FromStrErr: fmt::Display,
        T: TryFrom<i64> + FromStr<Err = FromStrErr>,
        <T as TryFrom<i64>>::Error: fmt::Display,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string containing json data")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            v.try_into().map_err(de::Error::custom)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            (*shellexpand::env(v).map_err(de::Error::custom)?)
                .parse::<T>()
                .map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(USizeVisitor::<T>(PhantomData::<T> {}))
}

/// Same as convert_numeric_with_shellexpand, but supports `Option<T>`.
pub fn convert_optional_numeric_with_shellexpand<'de, D, T, E>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    E: fmt::Display,
    T: TryFrom<i64> + FromStr<Err = E>,
    <T as TryFrom<i64>>::Error: fmt::Display,
{
    // define a visitor that deserializes
    // `ActualData` encoded as json within a string
    struct USizeVisitor<T: TryFrom<i64>>(PhantomData<T>);

    impl<'de, T, FromStrErr> de::Visitor<'de> for USizeVisitor<T>
    where
        FromStrErr: fmt::Display,
        T: TryFrom<i64> + FromStr<Err = FromStrErr>,
        <T as TryFrom<i64>>::Error: fmt::Display,
    {
        type Value = Option<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string containing json data")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v.try_into().map_err(de::Error::custom)?))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            if v.is_empty() {
                return Ok(None);
            }
            Ok(Some(
                (*shellexpand::env(v).map_err(de::Error::custom)?)
                    .parse::<T>()
                    .map_err(de::Error::custom)?,
            ))
        }
    }

    deserializer.deserialize_any(USizeVisitor::<T>(PhantomData::<T> {}))
}

/// Helper for serde macro so you can use shellexpand variables in the json configuration
/// files when the number is a numeric type.
pub fn convert_string_with_shellexpand<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    Ok((*(shellexpand::env(&value).map_err(de::Error::custom)?)).to_string())
}

/// Same as convert_string_with_shellexpand, but supports `Vec<String>`.
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

/// Same as convert_string_with_shellexpand, but supports `Option<String>`.
pub fn convert_optional_string_with_shellexpand<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    if let Some(value) = value {
        Ok(Some(
            (*(shellexpand::env(&value).map_err(de::Error::custom)?)).to_string(),
        ))
    } else {
        Ok(None)
    }
}

pub fn convert_data_size_with_shellexpand<'de, D, T, E>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    E: fmt::Display,
    T: TryFrom<i64> + FromStr<Err = E>,
    <T as TryFrom<i64>>::Error: fmt::Display,
{
    // define a visitor that deserializes
    // `ActualData` encoded as json within a string
    struct USizeVisitor<T: TryFrom<i64>>(PhantomData<T>);

    impl<'de, T, FromStrErr> de::Visitor<'de> for USizeVisitor<T>
    where
        FromStrErr: fmt::Display,
        T: TryFrom<i64> + FromStr<Err = FromStrErr>,
        <T as TryFrom<i64>>::Error: fmt::Display,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string containing json data")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            v.try_into().map_err(de::Error::custom)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            let expanded = (*shellexpand::env(v).map_err(de::Error::custom)?).to_string();
            let byte_size = Byte::parse_str(expanded, true).map_err(de::Error::custom)?;
            let byte_size_u128 = byte_size.as_u128();
            T::try_from(byte_size_u128.try_into().map_err(de::Error::custom)?)
                .map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(USizeVisitor::<T>(PhantomData::<T> {}))
}

pub fn convert_duration_with_shellexpand<'de, D, T, E>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    E: fmt::Display,
    T: TryFrom<i64> + FromStr<Err = E>,
    <T as TryFrom<i64>>::Error: fmt::Display,
{
    // define a visitor that deserializes
    // `ActualData` encoded as json within a string
    struct USizeVisitor<T: TryFrom<i64>>(PhantomData<T>);

    impl<'de, T, FromStrErr> de::Visitor<'de> for USizeVisitor<T>
    where
        FromStrErr: fmt::Display,
        T: TryFrom<i64> + FromStr<Err = FromStrErr>,
        <T as TryFrom<i64>>::Error: fmt::Display,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string containing json data")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            v.try_into().map_err(de::Error::custom)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            let expanded = (*shellexpand::env(v).map_err(de::Error::custom)?).to_string();
            let duration = parse_duration(&expanded).map_err(de::Error::custom)?;
            let duration_secs = duration.as_secs();
            T::try_from(duration_secs.try_into().map_err(de::Error::custom)?)
                .map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(USizeVisitor::<T>(PhantomData::<T> {}))
}
