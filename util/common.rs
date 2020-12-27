// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::result::Result;

use tokio::io::{Error, ErrorKind};

use proto::google::rpc::Status as GrpcStatus;
use tonic::Code;

pub fn result_to_grpc_status(result: Result<(), Error>) -> GrpcStatus {
    fn kind_to_grpc_code(kind: &ErrorKind) -> Code {
        match kind {
            ErrorKind::NotFound => Code::NotFound,
            ErrorKind::PermissionDenied => Code::PermissionDenied,
            ErrorKind::ConnectionRefused => Code::Unavailable,
            ErrorKind::ConnectionReset => Code::Unavailable,
            ErrorKind::ConnectionAborted => Code::Unavailable,
            ErrorKind::NotConnected => Code::Internal,
            ErrorKind::AddrInUse => Code::Internal,
            ErrorKind::AddrNotAvailable => Code::Internal,
            ErrorKind::BrokenPipe => Code::Internal,
            ErrorKind::AlreadyExists => Code::AlreadyExists,
            ErrorKind::WouldBlock => Code::Internal,
            ErrorKind::InvalidInput => Code::InvalidArgument,
            ErrorKind::InvalidData => Code::InvalidArgument,
            ErrorKind::TimedOut => Code::DeadlineExceeded,
            ErrorKind::WriteZero => Code::Internal,
            ErrorKind::Interrupted => Code::Aborted,
            ErrorKind::Other => Code::Internal,
            ErrorKind::UnexpectedEof => Code::Internal,
            _ => Code::Internal,
        }
    }
    match result {
        Ok(()) => GrpcStatus {
            code: Code::Ok as i32,
            message: "".to_string(),
            details: vec![],
        },
        Err(error) => GrpcStatus {
            code: kind_to_grpc_code(&error.kind()) as i32,
            message: format!("Error: {:?}", error),
            details: vec![],
        },
    }
}
