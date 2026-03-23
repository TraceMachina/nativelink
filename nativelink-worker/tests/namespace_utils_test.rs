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

#![cfg(target_os = "linux")]

use std::os::unix::process::CommandExt;
use std::process::Command;

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_worker::namespace_utils;
use pretty_assertions::assert_eq;

#[nativelink_test]
async fn test_namespaces_supported() -> Result<(), Error> {
    // This test is a smoke test to ensure that the namespace detection logic
    // runs without crashing. The result of this function is dependent on the
    // environment it is run in, so we don't assert the result.
    let _supported = namespace_utils::namespaces_supported();
    Ok(())
}

#[nativelink_test]
async fn test_configure_namespace() -> Result<(), Error> {
    if !namespace_utils::namespaces_supported() {
        return Ok(());
    }

    let mut command = Command::new("sh");
    command.args(["-c", "echo \"uid=$(id -u) pid=$$\"; exit 4"]);

    // SAFETY: `configure_namespace` is designed to be used with `pre_exec`.
    // It is async-signal-safe and will fork, configure the namespace in the
    // child, and the original child process will continue to execute the command.
    unsafe {
        command.pre_exec(namespace_utils::configure_namespace);
    }

    let output = command.output()?;
    assert_eq!(
        Some(4),
        output.status.code(),
        "Command failed to execute: {output:?}",
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // SAFETY: `geteuid` is always safe on POSIX systems.
    let expected_uid = unsafe { libc::geteuid() };
    let expected_output = format!("uid={expected_uid} pid=1\n");
    assert_eq!(stdout.trim(), expected_output.trim());

    Ok(())
}
