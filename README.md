# Turbo Cache

[![CI](https://github.com/allada/turbo-cache/workflows/CI/badge.svg)](https://github.com/allada/turbo-cache/actions/workflows/main.yml)

An extremely fast and efficient bazel cache service (CAS) written in rust.

The goals of this project are:
1. Stability - Things should work out of the box as expected
2. Efficiency - Don't waste time on inefficiencies &amp; low resource usage
3. Tested - Components should have plenty of tests &amp; each bug should be regression tested
4. Customers First - Design choices should be optimized for what customers want

## Overview

Turbo Cache is a project that implements the Bazel's [Remote Execution protocol](https://github.com/bazelbuild/remote-apis) (both CAS/Cache and remote execution portion).

When properly configured this project will provide extremely fast and efficient build cache for any systems that communicate using the [RBE protocol](https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto) and/or extremely fast, efficient and low foot-print remote execution capability.

Unix based operating systems and Windows are fully supported.

## TL;DR

If you have not updated Rust or Cargo recently, run:

`rustup update`

To compile and run the server:
```sh
# Install dependencies needed to compile Turbo Cache with bazel on
# worker machine (which is this machine).
apt install -y gcc g++ lld libssl-dev pkg-config python3

# Install cargo (if needed).
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# --release causes link-time-optmization to be enabled, which can take a while
# to compile, but will result in a much faster binary.
cargo run --release --bin cas -- ./config/examples/basic_cas.json
```
In a separate terminal session, run the following command to connect the running server launched above to Bazel or another RBE client:
```sh
bazel test //... \
  --remote_instance_name=main \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50051 \
  --remote_default_exec_properties=cpu_count=1
```
This will cause bazel to run the commands through an all-in-one `CAS`, `scheduler` and `worker`. See [here](https://github.com/allada/turbo-cache/tree/master/config) for configuration documentation and [here](https://github.com/allada/turbo-cache/tree/main/deployment-examples/terraform) for an example of multi-node cloud deployment example.

## Example Deployments
We currently have a few example deployments in [deployment-examples directory](https://github.com/allada/turbo-cache/tree/master/deployment-examples).

### Terraform
The [terraform deployment](https://github.com/allada/turbo-cache/tree/master/deployment-examples/terraform) is the currently preferred method as it leverages a lot of cloud resources to make everything much more robust.

The terraform deployment is very easy to setup and configure. This deployment will show off remote execution capabilities and cache capabilities.

## Status

This project can be considered ~stable~ and is currently used in production systems. Future API changes will be kept to a minimum.

## Build Requirements
We support building with Bazel or Cargo. Cargo **might** produce faster binaries because LTO (Link Time Optimization) is enabled for release versions, where Bazel currently does not support LTO for rust.

### Bazel requirements
* Bazel 6.4.0+
* gcc
* g++
* lld
* pkg-config
* python3

Runtime dependencies:
* `libssl-dev` or `libssl1.0-dev` (depending on your distro &amp; version)
#### Bazel building for deployment
```sh
# On Unix
bazel build cas

# On Windows
bazel build --config=windows cas
```
#### Bazel building for release
```sh
# On Unix
bazel build -c opt cas

# On Windows
bazel build --config=windows -c opt cas
```
> **Note**
> Failing to use the `-c opt` flag will result in a very slow binary (~10x slower).

These will place an executable in `./bazel-bin/cas/cas` that will start the service.

### Cargo requirements
* Cargo 1.70.0+
* `libssl-dev` package installed (ie: `apt install libssl-dev` or `yum install libssl-dev`)
#### Cargo building for deployment
```sh
cargo build
```
#### Cargo building for release
```sh
cargo build --release
```
> **Note**
> Failing to use the `-c opt` flag will result in a very slow binary (~10x slower).
> This is also significantly slower than building without `--release` because link-time-optimization
> is enabled by default with the flag.

### Configure

Configuration is done via a JSON file that is passed in as the first parameter to the `cas` program. See [here](https://github.com/allada/turbo-cache/tree/master/config) for more details and examples.

## How to update internal or external rust deps

In order to update external dependencies `Cargo.toml` is not the source of truth, instead7 these are tracked in `tools/cargo_shared.bzl`. It is done this way so both Bazel and Cargo can use the same dependencies that can be derived from the same source location.

All external dependencies are tracked in a generated `@crate_index` workspace and locked in `Cargo.Bazel.lock`. Some updates to `BUILD` files will require regenerating the `Cargo.toml` files. This is done with the `build_cargo_manifest.py`.

To regenerate the `@crate_index`:
```bash
# This will pin the new dependencies and generate new lock files.
CARGO_BAZEL_REPIN=1 bazel sync --only=crate_index
# This will update the Cargo.toml files with the new dependencies
# weather they are local or external.
python3 ./tools/build_cargo_manifest.py
```

## History

This project was first created due to frustration with similar projects not working or being extremely inefficient. Rust was chosen as the language to write it in because at the time rust was going through a revolution in the new-ish feature `async-await`. This made making multi-threading extremely simple when paired with a runtime (like [tokio](https://github.com/tokio-rs/tokio)) while still giving all the lifetime and other protections that Rust gives. This pretty much guarantees that we will never have crashes due to race conditions. This kind of project seemed perfect, since there is so much asynchronous activity happening and running them on different threads is most preferable. Other languages like `Go` are good candidates, but other similar projects rely heavily on channels and mutex locks which are cumbersome and have to be carefully designed by the developer. Rust doesn't have these issues, since the compiler will always tell you when the code you are writing might introduce undefined behavior. The last major reason is because Rust is extremely fast, +/- a few percent of C++ and has no garbage collection (like C++, but unlike `Java`, `Go`, or `Typescript`).

# License

Software is licensed under the Apache 2.0 License. Copyright 2020-2023 Trace Machina, Inc.
