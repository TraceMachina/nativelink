use nativelink_error::Error;
use walkdir::WalkDir;

#[test]
fn walkdir_source_error() {
    for entry in WalkDir::new("/bad/path") {
        let err: Error = entry.unwrap_err().into();
        let os_error = {
            #[cfg(unix)]
            {
                "No such file or directory (os error 2)"
            }
            #[cfg(windows)]
            {
                "The system cannot find the path specified. (os error 3)"
            }
        };
        assert_eq!(
            err.messages,
            vec![
                os_error,
                &format!("IO error for operation on /bad/path: {os_error}")
            ]
        );
    }
}

/// Redis failures reach REAPI clients as gRPC statuses, and `Bazel` treats
/// `INVALID_ARGUMENT` as permanent — no retry, build over. Anything caused by
/// the state of the `Redis` deployment rather than by the caller's request has
/// to map to a retryable status, or a routine failover fails CI.
mod redis_error_codes {
    use nativelink_error::{Code, Error};
    use redis::{ErrorKind, RedisError, ServerErrorKind};

    fn code_of(kind: ErrorKind) -> Code {
        let err: Error = RedisError::from((kind, "synthetic")).into();
        err.code
    }

    #[test]
    fn sentinel_failover_is_retryable() {
        // The exact shape observed killing a customer build mid-failover.
        assert_eq!(
            code_of(ErrorKind::MasterNameNotFoundBySentinel),
            Code::Unavailable
        );
        assert_eq!(
            code_of(ErrorKind::NoValidReplicasFoundBySentinel),
            Code::Unavailable
        );
    }

    #[test]
    fn transient_server_states_are_retryable() {
        for kind in [
            ServerErrorKind::ClusterDown,
            ServerErrorKind::MasterDown,
            ServerErrorKind::TryAgain,
            ServerErrorKind::BusyLoading,
            ServerErrorKind::ReadOnly,
        ] {
            assert_eq!(
                code_of(ErrorKind::Server(kind)),
                Code::Unavailable,
                "{kind:?} is transient and must be retryable"
            );
        }
        assert_eq!(
            code_of(ErrorKind::ClusterConnectionNotFound),
            Code::Unavailable
        );
    }

    #[test]
    fn dropped_connection_is_unavailable_and_timeout_is_deadline_exceeded() {
        let dropped: Error = RedisError::from(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset by peer",
        ))
        .into();
        assert_eq!(dropped.code, Code::Unavailable);

        let timed_out: Error = RedisError::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out",
        ))
        .into();
        assert_eq!(timed_out.code, Code::DeadlineExceeded);
    }

    /// A malformed reply is a fault on our side or the server's, not a bad
    /// argument from the client. This is the class that failed builds through
    /// the `FT.AGGREGATE` expiry race.
    #[test]
    fn protocol_faults_are_internal_not_invalid_argument() {
        assert_eq!(code_of(ErrorKind::Parse), Code::Internal);
        assert_eq!(code_of(ErrorKind::UnexpectedReturnType), Code::Internal);
    }

    /// Operator misconfiguration is the one case retrying genuinely cannot
    /// help, so it stays non-retryable.
    #[test]
    fn misconfiguration_stays_invalid_argument() {
        assert_eq!(
            code_of(ErrorKind::InvalidClientConfig),
            Code::InvalidArgument
        );
        assert_eq!(code_of(ErrorKind::EmptySentinelList), Code::InvalidArgument);
        assert_eq!(code_of(ErrorKind::RESP3NotSupported), Code::InvalidArgument);
    }

    #[test]
    fn auth_failures_are_permission_denied() {
        assert_eq!(
            code_of(ErrorKind::AuthenticationFailed),
            Code::PermissionDenied
        );
        assert_eq!(
            code_of(ErrorKind::Server(ServerErrorKind::NoPerm)),
            Code::PermissionDenied
        );
    }
}
