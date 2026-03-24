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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use nativelink_error::{make_err, Code, Error};
use pprof::protos::Message;
use pprof::ProfilerGuardBuilder;
use tracing::{info, warn};

use crate::spawn;
use crate::task::JoinHandleDropGuard;

/// Default CPU profiling duration in seconds.
const DEFAULT_PROFILE_SECONDS: u64 = 10;

/// Default sampling frequency in Hz.
const DEFAULT_FREQUENCY: i32 = 99;

/// CPU usage threshold (fraction of total cores) for auto-capture.
/// On a 64-core machine, 0.05 = 320% CPU (3.2 cores busy).
const AUTO_CAPTURE_CPU_THRESHOLD: f64 = 0.05;

/// How long to sample when auto-capturing.
const AUTO_CAPTURE_DURATION_SECS: u64 = 10;

/// How often to check CPU usage for auto-capture.
const AUTO_CAPTURE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// Cooldown after an auto-capture before capturing again.
const AUTO_CAPTURE_COOLDOWN: Duration = Duration::from_secs(120);

/// Maximum number of auto-captured profiles to keep on disk.
const AUTO_CAPTURE_MAX_FILES: usize = 10;

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

/// Get the process CPU usage as a fraction (0.0–1.0) by reading
/// /proc/self/stat on Linux or using rusage on other platforms.
fn get_cpu_usage() -> f64 {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        // Read /proc/self/stat for utime+stime, compare with wall clock.
        static PREV: std::sync::Mutex<Option<(u64, std::time::Instant)>> =
            std::sync::Mutex::new(None);
        let mut buf = String::new();
        if std::fs::File::open("/proc/self/stat")
            .and_then(|mut f| f.read_to_string(&mut buf))
            .is_err()
        {
            return 0.0;
        }
        let fields: Vec<&str> = buf.split_whitespace().collect();
        if fields.len() < 15 {
            return 0.0;
        }
        // Fields 13 and 14 are utime and stime in clock ticks.
        let ticks: u64 = fields[13].parse::<u64>().unwrap_or(0)
            + fields[14].parse::<u64>().unwrap_or(0);
        let now = std::time::Instant::now();
        let clk_tck = 100u64; // sysconf(_SC_CLK_TCK), almost always 100 on Linux
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get() as f64)
            .unwrap_or(1.0);

        let mut prev = PREV.lock().unwrap();
        let usage = if let Some((prev_ticks, prev_time)) = prev.as_ref() {
            let dt = now.duration_since(*prev_time).as_secs_f64();
            if dt < 0.1 {
                return 0.0;
            }
            let dticks = ticks.saturating_sub(*prev_ticks);
            (dticks as f64 / clk_tck as f64) / (dt * num_cpus)
        } else {
            0.0
        };
        *prev = Some((ticks, now));
        usage
    }
    #[cfg(not(target_os = "linux"))]
    {
        // On macOS, use a simpler heuristic: check if any thread
        // is consuming significant CPU via getrusage.
        static PREV: std::sync::Mutex<Option<(Duration, std::time::Instant)>> =
            std::sync::Mutex::new(None);

        let mut usage_val = libc::rusage {
            ru_utime: libc::timeval { tv_sec: 0, tv_usec: 0 },
            ru_stime: libc::timeval { tv_sec: 0, tv_usec: 0 },
            ru_maxrss: 0, ru_ixrss: 0, ru_idrss: 0, ru_isrss: 0,
            ru_minflt: 0, ru_majflt: 0, ru_nswap: 0, ru_inblock: 0,
            ru_oublock: 0, ru_msgsnd: 0, ru_msgrcv: 0, ru_nsignals: 0,
            ru_nvcsw: 0, ru_nivcsw: 0,
        };
        // SAFETY: getrusage with RUSAGE_SELF is always safe.
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage_val); }
        let total_usec = (usage_val.ru_utime.tv_sec as u64) * 1_000_000
            + (usage_val.ru_utime.tv_usec as u64)
            + (usage_val.ru_stime.tv_sec as u64) * 1_000_000
            + (usage_val.ru_stime.tv_usec as u64);
        let cpu_time = Duration::from_micros(total_usec);
        let now = std::time::Instant::now();
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get() as f64)
            .unwrap_or(1.0);

        let mut prev = PREV.lock().unwrap();
        let usage = if let Some((prev_cpu, prev_time)) = prev.as_ref() {
            let dt = now.duration_since(*prev_time).as_secs_f64();
            if dt < 0.1 {
                return 0.0;
            }
            let dcpu = cpu_time.saturating_sub(*prev_cpu).as_secs_f64();
            dcpu / (dt * num_cpus)
        } else {
            0.0
        };
        *prev = Some((cpu_time, now));
        usage
    }
}

/// Auto-capture: collect a profile and save SVG to disk.
fn auto_capture_profile(output_dir: &Path) -> Result<PathBuf, String> {
    let guard = ProfilerGuardBuilder::default()
        .frequency(DEFAULT_FREQUENCY)
        .build()
        .map_err(|e| format!("auto-capture: failed to start profiler: {e:?}"))?;

    std::thread::sleep(Duration::from_secs(AUTO_CAPTURE_DURATION_SECS));

    let report = guard
        .report()
        .build()
        .map_err(|e| format!("auto-capture: failed to build report: {e:?}"))?;

    let mut svg_buf = Vec::new();
    report
        .flamegraph(&mut svg_buf)
        .map_err(|e| format!("auto-capture: failed to generate flamegraph: {e:?}"))?;

    if svg_buf.is_empty() {
        return Err("auto-capture: empty flamegraph (no CPU samples)".into());
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let filename = format!("profile-{timestamp}.svg");
    let path = output_dir.join(&filename);
    std::fs::write(&path, &svg_buf)
        .map_err(|e| format!("auto-capture: failed to write {}: {e:?}", path.display()))?;

    // Rotate old files: keep only the most recent AUTO_CAPTURE_MAX_FILES.
    if let Ok(entries) = std::fs::read_dir(output_dir) {
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map_or(false, |n| n.starts_with("profile-") && n.ends_with(".svg"))
            })
            .collect();
        files.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
        for old in files.into_iter().skip(AUTO_CAPTURE_MAX_FILES) {
            let _ = std::fs::remove_file(old.path());
        }
    }

    Ok(path)
}

/// Background thread that monitors CPU usage and auto-captures profiles.
fn start_auto_capture_thread(output_dir: PathBuf, running: &'static AtomicBool) {
    std::thread::Builder::new()
        .name("pprof-auto-capture".into())
        .spawn(move || {
            let _ = std::fs::create_dir_all(&output_dir);
            // Prime the CPU usage tracker.
            get_cpu_usage();
            std::thread::sleep(AUTO_CAPTURE_CHECK_INTERVAL);

            while running.load(Ordering::Relaxed) {
                let cpu = get_cpu_usage();
                if cpu >= AUTO_CAPTURE_CPU_THRESHOLD {
                    info!(
                        cpu_pct = format!("{:.1}%", cpu * 100.0),
                        "auto-capture: CPU threshold exceeded, capturing profile"
                    );
                    match auto_capture_profile(&output_dir) {
                        Ok(path) => info!(
                            path = %path.display(),
                            "auto-capture: profile saved"
                        ),
                        Err(e) => warn!("auto-capture: {e}"),
                    }
                    // Cooldown to avoid flooding disk during sustained load.
                    std::thread::sleep(AUTO_CAPTURE_COOLDOWN);
                    // Re-prime after cooldown.
                    get_cpu_usage();
                }
                std::thread::sleep(AUTO_CAPTURE_CHECK_INTERVAL);
            }
        })
        .expect("failed to spawn pprof auto-capture thread");
}

/// Handler for `GET /debug/pprof/auto` — list auto-captured profiles.
async fn auto_list_handler() -> Response {
    let dir = PathBuf::from("/tmp/nativelink-pprof");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return (StatusCode::OK, "No auto-captured profiles yet.\n").into_response(),
    };
    let mut files: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("profile-") && name.ends_with(".svg") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    files.sort_by(|a, b| b.cmp(a));
    if files.is_empty() {
        return (StatusCode::OK, "No auto-captured profiles yet.\n").into_response();
    }
    let body = files.join("\n") + "\n";
    (StatusCode::OK, body).into_response()
}

/// Handler for `GET /debug/pprof/auto/:filename` — serve a captured profile.
async fn auto_serve_handler(
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> Response {
    // Prevent directory traversal.
    if filename.contains('/') || filename.contains("..") {
        return (StatusCode::BAD_REQUEST, "invalid filename").into_response();
    }
    let path = PathBuf::from("/tmp/nativelink-pprof").join(&filename);
    match std::fs::read(&path) {
        Ok(data) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
            data,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "profile not found").into_response(),
    }
}

/// Start the pprof HTTP server on the given port.
/// Returns a drop guard that keeps the server alive.
pub fn start_pprof_server(port: u16) -> Result<JoinHandleDropGuard<Result<(), Error>>, Error> {
    // Start auto-capture background thread.
    static AUTO_CAPTURE_RUNNING: AtomicBool = AtomicBool::new(true);
    start_auto_capture_thread(
        PathBuf::from("/tmp/nativelink-pprof"),
        &AUTO_CAPTURE_RUNNING,
    );

    let app = Router::new()
        .route("/debug/pprof/profile", get(profile_handler))
        .route("/debug/pprof/flamegraph", get(flamegraph_handler))
        .route("/debug/pprof/auto", get(auto_list_handler))
        .route("/debug/pprof/auto/{filename}", get(auto_serve_handler));

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
