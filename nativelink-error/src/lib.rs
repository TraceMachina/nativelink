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

use prost_types::TimestampError;

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

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Error {
    pub code: Code,
    pub messages: Vec<String>,
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

    pub fn set_code(mut self, code: Code) -> Self {
        self.code = code;
        self
    }

    #[inline]
    #[must_use]
    pub fn append<S: std::string::ToString>(mut self, msg: S) -> Self {
        self.messages.push(msg.to_string());
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
        other.map(|v| v.into())
    }

    pub fn to_std_err(self) -> std::io::Error {
        std::io::Error::new(self.code.into(), self.messages.join(" : "))
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

impl From<hex::FromHexError> for Error {
    fn from(err: hex::FromHexError) -> Self {
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
            code: err.kind().into(),
            messages: vec![err.to_string()],
        }
    }
}

impl From<Code> for Error {
    fn from(code: Code) -> Self {
        make_err!(code, "")
    }
}

impl From<tonic::transport::Error> for Error {
    fn from(error: tonic::transport::Error) -> Self {
        make_err!(Code::Internal, "{}", error.to_string())
    }
}

impl From<tonic::Status> for Error {
    fn from(status: tonic::Status) -> Self {
        make_err!(status.code().into(), "{}", status.to_string())
    }
}

impl From<Error> for tonic::Status {
    fn from(val: Error) -> Self {
        Self::new(val.code.into(), val.messages.join(" : "))
    }
}

pub trait ResultExt<T> {
    fn err_tip_with_code<F, S>(self, tip_fn: F) -> Result<T, Error>
    where
        Self: Sized,
        S: std::string::ToString,
        F: (std::ops::FnOnce(&Error) -> (Code, S)) + Sized;

    #[inline]
    fn err_tip<F, S>(self, tip_fn: F) -> Result<T, Error>
    where
        Self: Sized,
        S: std::string::ToString,
        F: (std::ops::FnOnce() -> S) + Sized,
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
        S: std::string::ToString,
        F: (std::ops::FnOnce(&Error) -> (Code, S)) + Sized,
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
        S: std::string::ToString,
        F: (std::ops::FnOnce(&Error) -> (Code, S)) + Sized,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive] // New Codes may be added in the future, so never exhaustively match!
pub enum Code {
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

impl From<i32> for Code {
    fn from(code: i32) -> Self {
        match code {
            x if x == Self::Ok as i32 => Self::Ok,
            x if x == Self::Cancelled as i32 => Self::Cancelled,
            x if x == Self::Unknown as i32 => Self::Unknown,
            x if x == Self::InvalidArgument as i32 => Self::InvalidArgument,
            x if x == Self::DeadlineExceeded as i32 => Self::DeadlineExceeded,
            x if x == Self::NotFound as i32 => Self::NotFound,
            x if x == Self::AlreadyExists as i32 => Self::AlreadyExists,
            x if x == Self::PermissionDenied as i32 => Self::PermissionDenied,
            x if x == Self::ResourceExhausted as i32 => Self::ResourceExhausted,
            x if x == Self::FailedPrecondition as i32 => Self::FailedPrecondition,
            x if x == Self::Aborted as i32 => Self::Aborted,
            x if x == Self::OutOfRange as i32 => Self::OutOfRange,
            x if x == Self::Unimplemented as i32 => Self::Unimplemented,
            x if x == Self::Internal as i32 => Self::Internal,
            x if x == Self::Unavailable as i32 => Self::Unavailable,
            x if x == Self::DataLoss as i32 => Self::DataLoss,
            x if x == Self::Unauthenticated as i32 => Self::Unauthenticated,
            _ => Self::Unknown,
        }
    }
}

impl From<tonic::Code> for Code {
    fn from(code: tonic::Code) -> Self {
        match code {
            tonic::Code::Ok => Self::Ok,
            tonic::Code::Cancelled => Self::Cancelled,
            tonic::Code::Unknown => Self::Unknown,
            tonic::Code::InvalidArgument => Self::InvalidArgument,
            tonic::Code::DeadlineExceeded => Self::DeadlineExceeded,
            tonic::Code::NotFound => Self::NotFound,
            tonic::Code::AlreadyExists => Self::AlreadyExists,
            tonic::Code::PermissionDenied => Self::PermissionDenied,
            tonic::Code::ResourceExhausted => Self::ResourceExhausted,
            tonic::Code::FailedPrecondition => Self::FailedPrecondition,
            tonic::Code::Aborted => Self::Aborted,
            tonic::Code::OutOfRange => Self::OutOfRange,
            tonic::Code::Unimplemented => Self::Unimplemented,
            tonic::Code::Internal => Self::Internal,
            tonic::Code::Unavailable => Self::Unavailable,
            tonic::Code::DataLoss => Self::DataLoss,
            tonic::Code::Unauthenticated => Self::Unauthenticated,
        }
    }
}

impl From<Code> for tonic::Code {
    fn from(val: Code) -> Self {
        match val {
            Code::Ok => Self::Ok,
            Code::Cancelled => Self::Cancelled,
            Code::Unknown => Self::Unknown,
            Code::InvalidArgument => Self::InvalidArgument,
            Code::DeadlineExceeded => Self::DeadlineExceeded,
            Code::NotFound => Self::NotFound,
            Code::AlreadyExists => Self::AlreadyExists,
            Code::PermissionDenied => Self::PermissionDenied,
            Code::ResourceExhausted => Self::ResourceExhausted,
            Code::FailedPrecondition => Self::FailedPrecondition,
            Code::Aborted => Self::Aborted,
            Code::OutOfRange => Self::OutOfRange,
            Code::Unimplemented => Self::Unimplemented,
            Code::Internal => Self::Internal,
            Code::Unavailable => Self::Unavailable,
            Code::DataLoss => Self::DataLoss,
            Code::Unauthenticated => Self::Unauthenticated,
        }
    }
}

impl From<std::io::ErrorKind> for Code {
    fn from(kind: std::io::ErrorKind) -> Self {
        match kind {
            std::io::ErrorKind::NotFound => Self::NotFound,
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted => Self::Unavailable,
            std::io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                Self::InvalidArgument
            }
            std::io::ErrorKind::TimedOut => Self::DeadlineExceeded,
            std::io::ErrorKind::Interrupted => Self::Aborted,
            std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::AddrInUse
            | std::io::ErrorKind::AddrNotAvailable
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::WriteZero
            | std::io::ErrorKind::Other
            | std::io::ErrorKind::UnexpectedEof => Self::Internal,
            _ => Self::Unknown,
        }
    }
}

impl From<Code> for std::io::ErrorKind {
    fn from(kind: Code) -> Self {
        match kind {
            Code::Aborted => Self::Interrupted,
            Code::AlreadyExists => Self::AlreadyExists,
            Code::DeadlineExceeded => Self::TimedOut,
            Code::InvalidArgument => Self::InvalidInput,
            Code::NotFound => Self::NotFound,
            Code::PermissionDenied => Self::PermissionDenied,
            Code::Unavailable => Self::ConnectionRefused,
            _ => Self::Other,
        }
    }
}
