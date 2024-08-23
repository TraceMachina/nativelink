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

//! Utilities for deserializing types with intermediate shell-expansion.

use std::fmt::Display;
use std::marker::PhantomData;
use std::str::FromStr;
use std::time::Duration;

use byte_unit::Byte;
use serde::{de, Deserialize, Deserializer};
use serde_with::{serde_as, DeserializeAs, Same, SerializeAs};

#[allow(rustdoc::private_intra_doc_links)]
/// Utility type that invokes shell-expansion if it detects a string.
///
/// # Examples
///
/// ```rust
/// # use serde::Deserialize;
/// # use serde_with::serde_as;
/// #
/// # use nativelink_config::deser::ShellExpand;
///
/// #[serde_as]
/// #[derive(Deserialize)]
/// struct Credentials {
///     username: String,
///
///     #[serde_as(as = "ShellExpand")]
///     password: String,
///
///     // Works with non-`String` types, too!
///     #[serde_as(as = "ShellExpand")]
///     pin: usize,
/// }
///
/// std::env::set_var("PASSWORD", "i-love-nativelink");
/// std::env::set_var("PIN", "1337");
///
/// let Credentials { username, password, pin } = serde_json5::from_str(r#"{
///     username: "tracemachina",
///     password: "$PASSWORD",
///     pin: "$PIN"
/// }"#).unwrap();
///
/// assert!(password.contains("nativelink"));
/// #
/// # assert_eq!(username, "tracemachina");
/// # assert_eq!(password, "i-love-nativelink");
/// # assert_eq!(pin, 1337);
/// ```
///
/// `ShellExpand` includes two type parameters you can use to control deserialization and parsing behaviors.
///
/// The first type parameter controls deserialization behavior. By default, values are deserialized via [`Deserialize`],
/// but you can specify any type that implements [`DeserializeAs`] to adjust deserialization.
///
/// ```rust
/// # use serde::Deserialize;
/// # use serde_with::{BoolFromInt, PickFirst, serde_as};
/// # use nativelink_config::deser::ShellExpand;
///
/// # #[derive(Debug)]
/// #[serde_as]
/// #[derive(Deserialize)]
/// struct Flags {
///     // Deserialize either from booleans or integers.
///     #[serde_as(as = "ShellExpand<PickFirst<(BoolFromInt, _)>, _>")]
///     verbose: bool,
/// }
///
/// let Flags { verbose: as_number } = serde_json5::from_str(r#"{ verbose: 1 }"#).unwrap();
/// let Flags { verbose: as_bool } = serde_json5::from_str(r#"{ verbose: true }"#).unwrap();
/// assert_eq!(as_number, as_bool);
/// # assert!(as_number);
/// # assert!(as_bool);
///
/// // Since we only modified the first parameter, shell-expansion parsing is unaffected; we can't parse a `bool`
/// // from a string containing the character `'1'`.
///
/// std::env::set_var("VERBOSE_INT", "1");
/// std::env::set_var("VERBOSE_BOOL", "true");
///
/// let ok = serde_json5::from_str::<Flags>(r#"{ verbose: "$VERBOSE_BOOL" }"#).unwrap();
/// let error = serde_json5::from_str::<Flags>(r#"{ verbose: "$VERBOSE_INT" }"#).unwrap_err(); // ERROR
/// # assert!(ok.verbose);
/// ```
///
/// Similarly, the second type parameter controls parsing behavior. By default, values are parsed via [`FromStr::from_str()`],
/// but you can specify any type that implements [`ShellExpandable`] to use a different parsing function.
///
/// ```rust
/// use std::time::Duration;
/// # use serde::Deserialize;
/// # use serde_with::serde_as;
/// # use nativelink_config::deser::ShellExpand;
///
/// # #[derive(Debug)]
/// #[serde_as]
/// #[derive(Deserialize)]
/// struct ConnectionSettings {
///     // Parse human-readable durations from shell-expanded strings.
///     #[serde_as(as = "ShellExpand<_, Duration>")]
///     ttl: usize,
/// }
///
/// std::env::set_var("TTL", "5m30s");
///
/// let ConnectionSettings { ttl } = serde_json5::from_str(r#"{ ttl: "$TTL" }"#).unwrap();
/// assert_eq!(ttl, 5 * 60 + 30);
///
/// // Deserialization behavior is unaffected
///
///  // ERROR
/// let error = serde_json5::from_str::<ConnectionSettings>(r#"{ ttl: { minutes: 5, seconds: 30 } }"#).unwrap_err();
/// ```
///
/// For convience, the types [`ShellExpandBytes`] and [`ShellExpandSeconds`] are exported as aliases for
/// `ShellExpand<T, Byte>` and `ShellExpand<T, Duration>`, respectively.
pub struct ShellExpand<De = Same, Conv = Same>(PhantomData<(De, Conv)>);

pub trait ShellExpandable<T> {
    fn convert<'de, D: Deserializer<'de>>(s: &str) -> Result<T, D::Error>;
}

impl<'de, De, Conv, T> DeserializeAs<'de, T> for ShellExpand<De, Conv>
where
    De: DeserializeAs<'de, T>,
    Conv: ShellExpandable<T>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// Helper enum that will catch strings in the `String` variant, and will deserialize everything else in the
        /// `Or` variant via `A::deserialize_as::<B>()`. As a consequence, any type that expects a `String`
        /// won't call `visit_str`, and will instead go through the shellexpand machinery.
        #[serde_as]
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOr<A, B> {
            /// Variant required to make `StringOr` "act" like it owns an `A` to satisfy the type-checker.
            #[serde(skip)]
            _A(PhantomData<A>),

            /// Catch-all variant for strings.
            String(String),

            /// Variant for "regular" deserialization.
            Or(
                #[serde_as(as = "A")]
                #[serde(bound(deserialize = "A: DeserializeAs<'de, B>"))]
                B,
            ),
        }

        match StringOr::<De, T>::deserialize(deserializer)? {
            StringOr::_A(_) => unreachable!("serde skips deserializing this variant"),
            StringOr::String(s) => {
                let expanded = shellexpand::env(&s).map_err(de::Error::custom)?;
                Conv::convert::<D>(&expanded)
            }
            StringOr::Or(t) => Ok(t),
        }
    }
}

impl<Se, _Conv, T: ?Sized> SerializeAs<T> for ShellExpand<Se, _Conv>
where
    Se: SerializeAs<T>,
{
    fn serialize_as<S>(source: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Se::serialize_as(source, serializer)
    }
}

impl<T> ShellExpandable<T> for Same
where
    T: FromStr<Err: Display>,
{
    fn convert<'de, D: Deserializer<'de>>(s: &str) -> Result<T, D::Error> {
        T::from_str(s).map_err(de::Error::custom)
    }
}

impl<T> ShellExpandable<T> for Byte
where
    u128: TryInto<T, Error: Display>,
{
    fn convert<'de, D: Deserializer<'de>>(s: &str) -> Result<T, D::Error> {
        let byte_size = Byte::parse_str(s, true).map_err(de::Error::custom)?;
        byte_size.as_u128().try_into().map_err(de::Error::custom)
    }
}

pub type ShellExpandBytes<De = Same> = ShellExpand<De, Byte>;

impl<T> ShellExpandable<T> for Duration
where
    u64: TryInto<T, Error: Display>,
{
    fn convert<'de, D: Deserializer<'de>>(s: &str) -> Result<T, D::Error> {
        let duration = humantime::parse_duration(s).map_err(de::Error::custom)?;
        duration.as_secs().try_into().map_err(de::Error::custom)
    }
}

pub type ShellExpandSeconds<De = Same> = ShellExpand<De, Duration>;
