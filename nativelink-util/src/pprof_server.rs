// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use nativelink_error::{make_err, Code, Error};
use pprof::protos::Message;
use pprof::ProfilerGuardBuilder;
use tracing::info;

use crate::spawn;
use crate::task::JoinHandleDropGuard;

/// Default CPU profiling duration in seconds.
const DEFAULT_PROFILE_SECONDS: u64 = 10;

/// Default sampling frequency in Hz.
const DEFAULT_FREQUENCY: i32 = 99;

#[derive(Debug, serde::Deserialize)]
struct ProfileParams {
    /// Duration to sample in seconds.
    seconds: Option<u64>,
    /// Output format: "pb" for protobuf, anything else for SVG flamegraph.
    format: Option<String>,
}

/// Handler for `GET /debug/pprof/profile`.
/// Returns SVG flamegraph by default, protobuf with `?format=pb`.
async fn profile_handler(Query(params): Query<ProfileParams>) -> Response {
    let seconds = params.seconds.unwrap_or(DEFAULT_PROFILE_SECONDS);
    let format = params.format.unwrap_or_default();

    let result = tokio::task::spawn_blocking(move || collect_profile(seconds, &format)).await;
    match result {
        Ok(Ok(resp)) => resp,
        Ok(Err(msg)) => (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("profiler task panicked: {e:?}"),
        )
            .into_response(),
    }
}

/// Handler for `GET /debug/pprof/flamegraph`.
/// Always returns SVG flamegraph.
async fn flamegraph_handler(Query(params): Query<ProfileParams>) -> Response {
    let seconds = params.seconds.unwrap_or(DEFAULT_PROFILE_SECONDS);

    let result = tokio::task::spawn_blocking(move || collect_profile(seconds, "svg")).await;
    match result {
        Ok(Ok(resp)) => resp,
        Ok(Err(msg)) => (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("profiler task panicked: {e:?}"),
        )
            .into_response(),
    }
}

/// Run the CPU profiler for `seconds` and return the result in the
/// requested format.
fn collect_profile(seconds: u64, format: &str) -> Result<Response, String> {
    let guard = ProfilerGuardBuilder::default()
        .frequency(DEFAULT_FREQUENCY)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
        .map_err(|e| format!("failed to start profiler: {e:?}"))?;

    std::thread::sleep(std::time::Duration::from_secs(seconds));

    let report = guard
        .report()
        .build()
        .map_err(|e| format!("failed to build report: {e:?}"))?;

    if format == "pb" {
        // Encode as pprof protobuf using prost 0.12 (pprof's own version).
        let profile = report
            .pprof()
            .map_err(|e| format!("failed to encode pprof protobuf: {e:?}"))?;
        let mut buf = Vec::with_capacity(profile.encoded_len());
        profile
            .encode(&mut buf)
            .map_err(|e| format!("failed to serialize protobuf: {e:?}"))?;
        Ok((
            StatusCode::OK,
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "application/octet-stream",
                ),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    "attachment; filename=\"profile.pb\"",
                ),
            ],
            buf,
        )
            .into_response())
    } else {
        // Default: SVG flamegraph.
        let mut svg_buf = Vec::new();
        report
            .flamegraph(&mut svg_buf)
            .map_err(|e| format!("failed to generate flamegraph: {e:?}"))?;
        Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
            svg_buf,
        )
            .into_response())
    }
}

/// Start the pprof HTTP server on the given port.
/// Returns a drop guard that keeps the server alive.
pub fn start_pprof_server(port: u16) -> Result<JoinHandleDropGuard<Result<(), Error>>, Error> {
    let app = Router::new()
        .route("/debug/pprof/profile", get(profile_handler))
        .route("/debug/pprof/flamegraph", get(flamegraph_handler));

    let addr: std::net::SocketAddr = ([0, 0, 0, 0], port).into();

    let guard = spawn!("pprof_http_server", async move {
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
            make_err!(
                Code::Internal,
                "failed to bind pprof HTTP server to {addr}: {e:?}"
            )
        })?;
        info!(%addr, "pprof HTTP server listening");
        axum::serve(listener, app).await.map_err(|e| {
            make_err!(
                Code::Internal,
                "pprof HTTP server exited with error: {e:?}"
            )
        })?;
        Ok(())
    });

    Ok(guard)
}
