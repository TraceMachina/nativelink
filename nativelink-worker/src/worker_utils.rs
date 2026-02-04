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

use core::hash::BuildHasher;
use core::str::from_utf8;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Cursor};
use std::process::Stdio;

use futures::future::try_join_all;
use nativelink_config::cas_server::WorkerProperty;
use nativelink_error::{Error, ResultExt, make_err, make_input_err};
use nativelink_proto::build::bazel::remote::execution::v2::platform::Property;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::ConnectWorkerRequest;
use tokio::process;
use tracing::info;

#[expect(clippy::future_not_send)] // TODO(jhpratt) remove this
pub async fn make_connect_worker_request<S: BuildHasher>(
    worker_id_prefix: String,
    worker_properties: &HashMap<String, WorkerProperty, S>,
    max_inflight_tasks: u64,
) -> Result<ConnectWorkerRequest, Error> {
    let mut futures = vec![];
    for (property_name, worker_property) in worker_properties {
        futures.push(async move {
            match worker_property {
                WorkerProperty::Values(values) => {
                    let mut props = Vec::with_capacity(values.len());
                    for value in values {
                        props.push(Property {
                            name: property_name.clone(),
                            value: value.clone(),
                        });
                    }
                    Ok(props)
                }
                WorkerProperty::QueryCmd(cmd) => {
                    let maybe_split_cmd = shlex::split(cmd);
                    let (command, args) = match &maybe_split_cmd {
                        Some(split_cmd) => (&split_cmd[0], &split_cmd[1..]),
                        None => {
                            return Err(make_input_err!(
                                "Could not parse the value of worker property: {}: '{}'",
                                property_name,
                                cmd
                            ));
                        }
                    };
                    let mut process = process::Command::new(command);
                    process.env_clear();
                    process.args(args);
                    process.stdin(Stdio::null());
                    let err_fn =
                        || format!("Error executing property_name {property_name} command");
                    info!(cmd, property_name, "Spawning process",);
                    let process_output = process.output().await.err_tip(err_fn)?;
                    if !process_output.status.success() {
                        return Err(make_err!(
                            process_output.status.code().unwrap().into(),
                            "{}",
                            err_fn()
                        ));
                    }
                    if !process_output.stderr.is_empty() {
                        eprintln!(
                            "{}",
                            from_utf8(&process_output.stderr).map_err(|e| make_input_err!(
                                "Failed to decode stderr to utf8 : {:?}",
                                e
                            ))?
                        );
                    }
                    let reader = BufReader::new(Cursor::new(process_output.stdout));

                    let mut props = vec![];
                    for value in reader.lines() {
                        props.push(Property {
                            name: property_name.clone(),
                            value: value
                                .err_tip(|| "Could split input by lines")?
                                .trim()
                                .to_string(),
                        });
                    }
                    Ok(props)
                }
            }
        });
    }

    Ok(ConnectWorkerRequest {
        worker_id_prefix,
        properties: try_join_all(futures).await?.into_iter().flatten().collect(),
        max_inflight_tasks,
    })
}
