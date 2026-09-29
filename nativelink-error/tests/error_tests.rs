use nativelink_error::{Code, Error};
use walkdir::WalkDir;

// A transport-layer failure (mid-stream connection reset) is reported by tonic
// as `Code::Unknown` "transport error", but it is transient and must reach the
// client as the retryable `Unavailable` — otherwise big cacheable uploads are
// silently dropped when a shard resets the connection mid-write.
#[test]
fn transport_unknown_status_maps_to_unavailable() {
    let status = tonic::Status::unknown("transport error");
    let err: Error = status.into();
    assert_eq!(err.code, Code::Unavailable);
}

// A genuinely-unmapped `Unknown` (no transport origin) is preserved as-is.
#[test]
fn genuine_unknown_status_is_preserved() {
    let status = tonic::Status::unknown("some application-level failure");
    let err: Error = status.into();
    assert_eq!(err.code, Code::Unknown);
}

// A transport-error status that passed through this crate's own
// `From<Error> for Status` re-encoding (message segments joined with " : ")
// must still be recognized after the source chain is gone — the multi-hop
// proxy case the fallback exists for.
#[test]
fn proxied_transport_error_with_appended_context_maps_to_unavailable() {
    let status = tonic::Status::unknown("transport error : while writing to upstream shard");
    let err: Error = status.into();
    assert_eq!(err.code, Code::Unavailable);
}

// The message match is anchored: an app-level `Unknown` that merely mentions
// "transport error" mid-message must NOT be reclassified as retryable.
#[test]
fn unknown_mentioning_transport_error_mid_message_is_preserved() {
    let status = tonic::Status::unknown("upstream proxy saw a transport error and gave up");
    let err: Error = status.into();
    assert_eq!(err.code, Code::Unknown);
}

// The downcast walk, tested in isolation from the message fallback: a
// `Status` whose SOURCE CHAIN carries a real `tonic::transport::Error` maps
// to `Unavailable` even when its message matches neither anchor. The
// transport error is produced the honest way (a connect to a port with no
// listener, bound-then-dropped so nothing can race onto it) and attached
// via `Status::set_source` — the shape a middleware produces when it wraps
// a transport failure with its own message. Going through tonic's own
// `Status::from_error` instead would not reach our walk: tonic already
// maps the shapes it recognizes to `Unavailable` before we ever see them.
#[expect(
    clippy::disallowed_methods,
    reason = "obtaining a real transport error requires driving a connect on a runtime"
)]
#[test]
fn transport_error_in_source_chain_maps_to_unavailable() {
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
    let transport_err = runtime.block_on(async {
        let dead_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{dead_port}"))
            .unwrap()
            .connect_timeout(core::time::Duration::from_secs(5))
            .connect()
            .await
            .expect_err("connect to a dropped listener must fail")
    });
    let mut status = tonic::Status::unknown("middleware wrapped the failure with its own text");
    status.set_source(std::sync::Arc::new(transport_err));
    let err: Error = status.into();
    assert_eq!(err.code, Code::Unavailable);
}

// Pin the round-trip that produces the proxied shape above: converting an
// `Unavailable` transport Error back to a Status and re-ingesting it must
// stay `Unavailable` with the anchored prefix intact.
#[test]
fn transport_error_round_trip_stays_unavailable() {
    let err: Error = tonic::Status::unknown("transport error").into();
    assert_eq!(err.code, Code::Unavailable);
    let with_context = err.append("while writing to upstream shard");
    let status: tonic::Status = with_context.into();
    let round_tripped: Error = status.into();
    assert_eq!(round_tripped.code, Code::Unavailable);
}

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
