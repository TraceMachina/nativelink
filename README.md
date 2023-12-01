# Native Link

[![CI](https://github.com/tracemachina/native-link/workflows/CI/badge.svg)](https://github.com/tracemachina/native-link/actions/workflows/main.yml)

Native link is an extremely (blazingly?) fast and efficient build cache and
remote executor for systems that communicate using the [Remote execution
protocol](https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto) such as [Bazel](https://bazel.build), [Buck2](https://buck2.build) and [Goma](https://chromium.googlesource.com/infra/goma/client/).

Supports Unix based operating systems and Windows.

## ❄️ Installing with Nix

**Installation requirements:**

* Nix with [flakes](https://nixos.wiki/wiki/Flakes) enabled

This build does not require cloning the repository, but you need to provide a
config file, for instance the one at [native-link-config/examples/basic_cas.json](./native-link-config/examples/basic_cas.json).

The following command builds and runs Native Link in release (optimized) mode:

```sh
nix run github:TraceMachina/native-link ./basic_cas.json
```

For use in production pin the executable to a specific revision:

```sh
nix run github:TraceMachina/native-link/<revision> ./basic_cas.json
```

## 📦 Using the OCI image

See the published [OCI images](https://github.com/TraceMachina/native-link/pkgs/container/native-link)
for pull commands.

Images are tagged by nix derivation hash. The most recently pushed image
corresponds to the `main` branch. Images are signed by the GitHub action that
produced the image:

```sh
# Get the tag for the latest commit
export LATEST=$(nix eval github:TraceMachina/native-link#image.imageTag --raw)

# Verify the signature
cosign verify ghcr.io/TraceMachina/native-link:${LATEST} \
    --certificate-identity=https://github.com/TraceMachina/native-link/.github/workflows/image.yaml@refs/heads/main \
    --certificate-oidc-issuer=https://token.actions.githubusercontent.com
```

> [TIP!]
> The images are reproducible on `X86_64-unknown-linux-gnu`. If you're on such a
> system you can produce a binary-identical image by building the `.#image`
> flake output locally. Make sure that your `git status` is completely clean and
> aligned with the commit you want to reproduce. Otherwise the image will be
> tainted with a `"dirty"` revision label.

## 🌱 Building with Bazel

**Build requirements:**

* Bazel 6.4.0+
* A recent C++ toolchain with LLD as linker

> [!TIP]
> This build supports Nix/direnv which provides Bazel but no C++ toolchain
> (yet).

The following commands place an executable in `./bazel-bin/cas/cas` and start
the service:

```sh
# Unoptimized development build on Unix
bazel run cas

# Optimized release build on Unix
bazel run -c opt cas

# Unoptimized development build on Windows
bazel run --config=windows cas

# Optimized release build on Windows
bazel run --config=windows -c opt cas
```

## 🦀 Building with Cargo

**Build requirements:**

* Cargo 1.73.0+
* A recent C++ toolchain with LLD as linker

> [!TIP]
> This build supports Nix/direnv which provides Cargo but no C++
> toolchain/stdenv (yet).

```bash
# Unoptimized development build
cargo run --bin cas -- ./native-link-config/examples/basic_cas.json

# Optimized release build
cargo run --release --bin cas -- ./native-link-config/examples/basic_cas.json
```

## 🧪 Evaluating Native Link

Once you've built Native Link and have an instance running with the
`basic_cas.json` configuration, launch a separate terminal session and run the
following command to connect the running server launched above to Bazel or
another RBE client:

```sh
bazel test //... \
  --remote_instance_name=main \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50051 \
  --remote_default_exec_properties=cpu_count=1
```

This causes bazel to run the commands through an all-in-one `CAS`, `scheduler`
and `worker`.

## ⚙️ Configuration

The `cas` executable reads a JSON file as it's only parameter. See [native-link-config](./native-link-config)
for more details and examples.

## 🚀 Example Deployments

You can find a few example deployments in the [deployment-examples directory](./deployment-examples).

See the [terraform deployments](./deployment-examples/terraform) for an example
deployments that show off remote execution and cache capabilities.

## 🏺 History

This project was first created due to frustration with similar projects not
working or being extremely inefficient. Rust was chosen as the language to write
it in because at the time Rust was going through a revolution in the new-ish
feature `async-await`. This made making multi-threading extremely simple when
paired with a runtime like [tokio](https://github.com/tokio-rs/tokio) while
still giving all the lifetime and other protections that Rust gives. This pretty
much guarantees that we will never have crashes due to race conditions. This
kind of project seemed perfect, since there is so much asynchronous activity
happening and running them on different threads is most preferable. Other
languages like `Go` are good candidates, but other similar projects rely heavily
on channels and mutex locks which are cumbersome and have to be carefully
designed by the developer. Rust doesn't have these issues, since the compiler
will always tell you when the code you are writing might introduce undefined
behavior. The last major reason is because Rust is extremely fast, +/- a few
percent of C++ and has no garbage collection (like C++, but unlike `Java`, `Go`,
or `Typescript`).

## 📜 License

Copyright 2020-2023 Trace Machina, Inc.

Licensed under the Apache 2.0 License, SPDX identifier `Apache-2.0`.
