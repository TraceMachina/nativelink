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

use std::convert::Into;

use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
};
use prost_types::TimestampError;
use serde::{Deserialize, Serialize};
// Reexport of tonic's error codes which we use as "nativelink_error::Code".
pub use tonic::Code;

#[macro_export]
macro_rules! make_err {
    ($code:expr, $($arg:tt)+) => {{
        $crate::Error::new(
            $code,
            format!("{}", format_args!($($arg)+)),
        )
    }};
}

#[macro_export]
macro_rules! make_input_err {
    ($($arg:tt)+) => {{
        $crate::make_err!($crate::Code::InvalidArgument, $($arg)+)
    }};
}

#[macro_export]
macro_rules! error_if {
    ($cond:expr, $($arg:tt)+) => {{
        if $cond {
            Err($crate::make_err!($crate::Code::InvalidArgument, $($arg)+))?;
        }
    }};
}

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct Error {
    #[serde(with = "CodeDef")]
    pub code: Code,
    pub messages: Vec<String>,
}

impl MetricsComponent for Error {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        self.to_string().publish(kind, field_metadata)
    }
}

impl Error {
    pub fn new(code: Code, msg: String) -> Self {
        let mut msgs = Vec::with_capacity(1);
        if !msg.is_empty() {
            msgs.push(msg);
        }
        Self {
            code,
            messages: msgs,
        }
    }

    #[inline]
    #[must_use]
    pub fn append<S: Into<String>>(mut self, msg: S) -> Self {
        self.messages.push(msg.into());
        self
    }

    #[must_use]
    pub fn merge<E: Into<Self>>(mut self, other: E) -> Self {
        let mut other: Self = other.into();
        // This will help with knowing which messages are tied to different errors.
        self.messages.push("---".to_string());
        self.messages.append(&mut other.messages);
        self
    }

    #[must_use]
    pub fn merge_option<T: Into<Self>, U: Into<Self>>(
        this: Option<T>,
        other: Option<U>,
    ) -> Option<Self> {
        if let Some(this) = this {
            if let Some(other) = other {
                return Some(this.into().merge(other));
            }
            return Some(this.into());
        }
        other.map(Into::into)
    }

    pub fn to_std_err(self) -> std::io::Error {
        std::io::Error::new(self.code.into_error_kind(), self.messages.join(" : "))
    }

    pub fn message_string(&self) -> String {
        self.messages.join(" : ")
    }
}

impl std::error::Error for Error {}

impl From<Error> for nativelink_proto::google::rpc::Status {
    fn from(val: Error) -> Self {
        Self {
            code: val.code as i32,
            message: val.message_string(),
            details: vec![],
        }
    }
}

impl From<nativelink_proto::google::rpc::Status> for Error {
    fn from(val: nativelink_proto::google::rpc::Status) -> Self {
        Self {
            code: val.code.into(),
            messages: vec![val.message],
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A manual impl to reduce the noise of frequently empty fields.
        let mut builder = f.debug_struct("Error");

        builder.field("code", &self.code);

        if !self.messages.is_empty() {
            builder.field("messages", &self.messages);
        }

        builder.finish()
    }
}

impl From<prost::DecodeError> for Error {
    fn from(err: prost::DecodeError) -> Self {
        make_err!(Code::Internal, "{}", err.to_string())
    }
}

impl From<prost::EncodeError> for Error {
    fn from(err: prost::EncodeError) -> Self {
        make_err!(Code::Internal, "{}", err.to_string())
    }
}

impl From<prost::UnknownEnumValue> for Error {
    fn from(err: prost::UnknownEnumValue) -> Self {
        make_err!(Code::Internal, "{}", err.to_string())
    }
}

impl From<std::num::TryFromIntError> for Error {
    fn from(err: std::num::TryFromIntError) -> Self {
        make_err!(Code::InvalidArgument, "{}", err.to_string())
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(err: tokio::task::JoinError) -> Self {
        make_err!(Code::Internal, "{}", err.to_string())
    }
}

impl From<std::num::ParseIntError> for Error {
    fn from(err: std::num::ParseIntError) -> Self {
        make_err!(Code::InvalidArgument, "{}", err.to_string())
    }
}

impl From<std::convert::Infallible> for Error {
    fn from(_err: std::convert::Infallible) -> Self {
        // Infallible is an error type that can never happen.
        unreachable!();
    }
}

impl From<TimestampError> for Error {
    fn from(err: TimestampError) -> Self {
        make_err!(Code::InvalidArgument, "{}", err)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self {
            code: err.kind().into_code(),
            messages: vec![err.to_string()],
        }
    }
}

impl From<fred::error::Error> for Error {
    fn from(error: fred::error::Error) -> Self {
        use fred::error::ErrorKind::{
            Auth, Backpressure, Canceled, Cluster, Config, InvalidArgument, InvalidCommand,
            NotFound, Parse, Protocol, Routing, Sentinel, Timeout, Tls, Unknown, Url, IO,
        };

        // Conversions here are based on https://grpc.github.io/grpc/core/md_doc_statuscodes.html.
        let code = match error.kind() {
            Config | InvalidCommand | InvalidArgument | Url => Code::InvalidArgument,
            IO | Protocol | Tls | Cluster | Parse | Sentinel | Routing => Code::Internal,
            Auth => Code::PermissionDenied,
            Canceled => Code::Aborted,
            Unknown => Code::Unknown,
            Timeout => Code::DeadlineExceeded,
            NotFound => Code::NotFound,
            Backpressure => Code::Unavailable,
        };

        make_err!(code, "{error}")
    }
}

impl From<tonic::Status> for Error {
    fn from(status: tonic::Status) -> Self {
        make_err!(status.code(), "{}", status.to_string())
    }
}

impl From<Error> for tonic::Status {
    fn from(val: Error) -> Self {
        Self::new(val.code, val.messages.join(" : "))
    }
}

pub trait ResultExt<T> {
    fn err_tip_with_code<F, S>(self, tip_fn: F) -> Result<T, Error>
    where
        Self: Sized,
        S: ToString,
        F: (FnOnce(&Error) -> (Code, S)) + Sized;

    #[inline]
    fn err_tip<F, S>(self, tip_fn: F) -> Result<T, Error>
    where
        Self: Sized,
        S: ToString,
        F: (FnOnce() -> S) + Sized,
    {
        self.err_tip_with_code(|e| (e.code, tip_fn()))
    }

    fn merge<U>(self, _other: Result<U, Error>) -> Result<U, Error>
    where
        Self: Sized,
    {
        unreachable!();
    }
}

impl<T, E: Into<Error>> ResultExt<T> for Result<T, E> {
    #[inline]
    fn err_tip_with_code<F, S>(self, tip_fn: F) -> Result<T, Error>
    where
        Self: Sized,
        S: ToString,
        F: (FnOnce(&Error) -> (Code, S)) + Sized,
    {
        self.map_err(|e| {
            let mut error: Error = e.into();
            let (code, message) = tip_fn(&error);
            error.code = code;
            error.messages.push(message.to_string());
            error
        })
    }

    fn merge<U>(self, other: Result<U, Error>) -> Result<U, Error>
    where
        Self: Sized,
    {
        if let Err(e) = self {
            let mut e: Error = e.into();
            if let Err(other_err) = other {
                let mut other_err: Error = other_err;
                // This will help with knowing which messages are tied to different errors.
                e.messages.push("---".to_string());
                e.messages.append(&mut other_err.messages);
            }
            return Err(e);
        }
        other
    }
}

impl<T> ResultExt<T> for Option<T> {
    #[inline]
    fn err_tip_with_code<F, S>(self, tip_fn: F) -> Result<T, Error>
    where
        Self: Sized,
        S: ToString,
        F: (FnOnce(&Error) -> (Code, S)) + Sized,
    {
        self.ok_or_else(|| {
            let mut error = Error {
                code: Code::Internal,
                messages: vec![],
            };
            let (code, message) = tip_fn(&error);
            error.code = code;
            error.messages.push(message.to_string());
            error
        })
    }
}

trait CodeExt {
    fn into_error_kind(self) -> std::io::ErrorKind;
}

impl CodeExt for Code {
    fn into_error_kind(self) -> std::io::ErrorKind {
        match self {
            Code::Aborted => std::io::ErrorKind::Interrupted,
            Code::AlreadyExists => std::io::ErrorKind::AlreadyExists,
            Code::DeadlineExceeded => std::io::ErrorKind::TimedOut,
            Code::InvalidArgument => std::io::ErrorKind::InvalidInput,
            Code::NotFound => std::io::ErrorKind::NotFound,
            Code::PermissionDenied => std::io::ErrorKind::PermissionDenied,
            Code::Unavailable => std::io::ErrorKind::ConnectionRefused,
            _ => std::io::ErrorKind::Other,
        }
    }
}

trait ErrorKindExt {
    fn into_code(self) -> Code;
}

impl ErrorKindExt for std::io::ErrorKind {
    fn into_code(self) -> Code {
        match self {
            Self::NotFound => Code::NotFound,
            Self::PermissionDenied => Code::PermissionDenied,
            Self::ConnectionRefused | Self::ConnectionReset | Self::ConnectionAborted => {
                Code::Unavailable
            }
            Self::AlreadyExists => Code::AlreadyExists,
            Self::InvalidInput | Self::InvalidData => Code::InvalidArgument,
            Self::TimedOut => Code::DeadlineExceeded,
            Self::Interrupted => Code::Aborted,
            Self::NotConnected
            | Self::AddrInUse
            | Self::AddrNotAvailable
            | Self::BrokenPipe
            | Self::WouldBlock
            | Self::WriteZero
            | Self::Other
            | Self::UnexpectedEof => Code::Internal,
            _ => Code::Unknown,
        }
    }
}

// Serde definition for tonic::Code. See: https://serde.rs/remote-derive.html
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(remote = "Code")]
pub enum CodeDef {
    Ok = 0,
    Cancelled = 1,
    Unknown = 2,
    InvalidArgument = 3,
    DeadlineExceeded = 4,
    NotFound = 5,
    AlreadyExists = 6,
    PermissionDenied = 7,
    ResourceExhausted = 8,
    FailedPrecondition = 9,
    Aborted = 10,
    OutOfRange = 11,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    DataLoss = 15,
    Unauthenticated = 16,
    // NOTE: Additional codes must be added to stores.rs in ErrorCodes and also
    // in both match statements in retry.rs.
}
