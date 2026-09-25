// Copyright 2026 The NativeLink Authors. All rights reserved.
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

//! Buck2-only execution-container capture, held through action cleanup.
//! The helper initiates authenticated uploads; `NativeLink` never opens a public
//! file server or grants a browser access to the worker API.

use core::time::Duration;
use std::path::Path;
use std::process::Stdio;

use nativelink_config::cas_server::Buck2FileCaptureConfig;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::RequestMetadata;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use uuid::Uuid;

#[derive(Debug)]
pub struct Buck2FileCapture {
    child: Child,
    finish_timeout: Duration,
}

impl Buck2FileCapture {
    pub fn eligible(metadata: Option<&RequestMetadata>) -> bool {
        metadata.is_some_and(|metadata| {
            !metadata.tool_invocation_id.is_empty()
                && metadata
                    .tool_details
                    .as_ref()
                    .is_some_and(|tool| tool.tool_name.eq_ignore_ascii_case("buck2"))
        })
    }
    /// Only authenticated scheduler metadata identifying Buck2 may start a
    /// capture. A missing invocation or another tool leaves the old flow intact.
    pub async fn start(
        config: &Buck2FileCaptureConfig,
        metadata: Option<&RequestMetadata>,
        worker: &str,
        attempt: &str,
        action_digest: &str,
    ) -> Result<Option<Self>, Error> {
        let Some(metadata) = metadata.filter(|metadata| Self::eligible(Some(metadata))) else {
            return Ok(None);
        };
        let session = Uuid::new_v4().to_string();
        let finalize_seconds = if config.finalize_timeout_s == 0 {
            120
        } else {
            config.finalize_timeout_s
        };
        let configuration = json!({
            "root": "/",
            "stateDirectory": Path::new(&config.state_directory).join(&session),
            "protectedPaths": config.protected_paths,
            "internalPaths": config.internal_paths,
            "gateway": config.gateway,
            "tokenFile": config.token_file,
            "finalizeSeconds": finalize_seconds,
            "session": {
                "id": session,
                "tool": "buck2",
                "build": metadata.tool_invocation_id,
                "action": action_digest,
                "attempt": attempt,
                "worker": worker,
                "container": config.container_id,
            },
        });
        let mut body = serde_json::to_vec(&configuration)
            .map_err(|err| Error::from_std_err(Code::Internal, &err))?;
        body.push(b'\n');
        let mut command = Command::new(&config.executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env_clear()
            .kill_on_drop(true);
        // Customer-provided trust roots are configuration, not action env.
        for name in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let startup = async move {
            let mut child = command
                .spawn()
                .err_tip(|| "Starting Buck2 file capture helper")?;
            child
                .stdin
                .as_mut()
                .err_tip(|| "Capture helper has no control pipe")?
                .write_all(&body)
                .await
                .err_tip(|| "Configuring Buck2 file capture")?;
            let mut stdout = child
                .stdout
                .take()
                .err_tip(|| "Capture helper has no readiness pipe")?;
            let mut ready = [0_u8; 6];
            stdout
                .read_exact(&mut ready)
                .await
                .err_tip(|| "Waiting for Buck2 capture readiness")?;
            if &ready != b"ready\n" {
                return Err(make_err!(
                    Code::FailedPrecondition,
                    "Invalid Buck2 capture readiness acknowledgement"
                ));
            }
            Ok(Some(Self {
                child,
                // The helper reserves five seconds to acknowledge a partial
                // final scan, plus five seconds for shutdown/reaping here.
                finish_timeout: Duration::from_secs(finalize_seconds as u64 + 10),
            }))
        };
        tokio::time::timeout(Duration::from_secs(30), startup)
            .await
            .err_tip(|| "Buck2 capture registration timed out")?
    }

    /// Closing the control pipe asks the helper to stop live work, capture a
    /// final revision and wait for durable acknowledgement. This must run before
    /// deleting action files or reporting the single-use worker as completed.
    pub async fn finish(mut self) -> Result<(), Error> {
        drop(self.child.stdin.take());
        if let Ok(status) = tokio::time::timeout(self.finish_timeout, self.child.wait()).await {
            let status = status.err_tip(|| "Reaping Buck2 capture helper")?;
            if status.success() {
                Ok(())
            } else {
                Err(make_err!(
                    Code::Unavailable,
                    "Buck2 file capture ended with {status}; archive may be partial"
                ))
            }
        } else {
            self.child
                .kill()
                .await
                .err_tip(|| "Stopping timed-out Buck2 capture helper")?;
            Err(make_err!(
                Code::DeadlineExceeded,
                "Buck2 capture finalization deadline exceeded; archive is partial"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use nativelink_proto::build::bazel::remote::execution::v2::ToolDetails;

    use super::*;

    fn metadata(tool: &str, invocation: &str) -> RequestMetadata {
        RequestMetadata {
            tool_details: Some(ToolDetails {
                tool_name: tool.to_string(),
                ..Default::default()
            }),
            tool_invocation_id: invocation.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn only_buck2_with_an_invocation_is_eligible() {
        assert!(!Buck2FileCapture::eligible(None));
        for tool in ["", "bazel", "pants", "recc"] {
            assert!(!Buck2FileCapture::eligible(Some(&metadata(
                tool,
                "invocation"
            ))));
        }
        assert!(!Buck2FileCapture::eligible(Some(&metadata("buck2", ""))));
        assert!(Buck2FileCapture::eligible(Some(&metadata(
            "buck2",
            "invocation"
        ))));
        assert!(Buck2FileCapture::eligible(Some(&metadata(
            "Buck2",
            "invocation"
        ))));
    }

    #[cfg(target_family = "unix")]
    #[nativelink_macro::nativelink_test]
    async fn non_buck2_does_not_launch_the_helper() -> Result<(), Error> {
        let config = Buck2FileCaptureConfig {
            executable: "/nonexistent-capture-helper".to_string(),
            gateway: String::new(),
            token_file: String::new(),
            state_directory: String::new(),
            container_id: String::new(),
            protected_paths: vec![],
            internal_paths: vec![],
            finalize_timeout_s: 0,
        };
        for request in [
            None,
            Some(metadata("bazel", "invocation")),
            Some(metadata("buck2", "")),
        ] {
            assert!(
                Buck2FileCapture::start(&config, request.as_ref(), "worker", "attempt", "digest")
                    .await?
                    .is_none()
            );
        }
        Ok(())
    }

    #[cfg(target_family = "unix")]
    #[nativelink_macro::nativelink_test]
    async fn unresponsive_capture_has_a_bounded_shutdown() -> Result<(), Error> {
        // Nix supplies coreutils through PATH and has no /bin/sleep.
        let child = Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .err_tip(|| "Starting unresponsive test helper")?;
        let capture = Buck2FileCapture {
            child,
            finish_timeout: Duration::from_millis(20),
        };
        let result = tokio::time::timeout(Duration::from_secs(2), capture.finish())
            .await
            .err_tip(|| "Capture deadline did not bound shutdown")?;
        assert_eq!(result.unwrap_err().code, Code::DeadlineExceeded);
        Ok(())
    }
}
