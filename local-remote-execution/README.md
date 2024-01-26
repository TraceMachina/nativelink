# Local Remote Execution

NativeLink's Local Remote Execution is a framework to build and rapidly iterate
custom toolchain setups that are transparent, fully hermetic and reproducible
across machines of the same system architecture. When used in conjunction with Nix,
Local Remote Execution recreates the remote execution environment in your local
development environment. Working with LRE lets you seamlessly switch between remote
and local builds while reusing the same cache.

You must clone the repo to use Local Remote Execution. However, the simplest way to use Local Remote Execution today is to [install and start NativeLink with Cargo](https://github.com/TraceMachina/nativelink?tab=readme-ov-file#-installing-with-cargo).
## Pre-Requisites


- Bazel 7.0.0 or later or another build system that implements [Bazel's RBE](https://bazel.build/remote/rbe)
- Rust 1.73.0 or later
- Nix 2.19.0 or later


## Setting up Local Remote Execution

> [!Note]
> Local Remote Execution has only been tested on `x86_64-linux`.

1. Enable a [Flake-based](https://nixos.wiki/wiki/Development_environment_with_nix-shell) Nix Shell within the NativeLink root directory.

```shell
nix develop
```
2. Create an [OCI image](https://opencontainers.org/) containing the toolchains:

```shell
generate-toolchains
```

You should see terminal output beginning with:
`++ git rev-parse --show-toplevel`


The `generate-toolchains` command creates an OCI image from a nix `stdenv` and
generates toolchain configs from it. The resulting [generated C++ toolchains](
./generated/cc/BUILD) have all tools pinned to specific derivations in
`/nix/store/*`. These paths mirror the ones that you fetched when entering the
flake development environment, i.e. the tools in the container and in your local
environment are the same.

3. You can now build targets with the generated toolchain configs. To build the `hello_lre` target in the [examples](./examples/) directory:

```shell
bazel run --config=lre //local-remote-execution/examples:hello_lre
```
🎉 That's it! You've built a target with Local Remote Execution using NativeLink. 🎉

## Switching to Remote Execution

If you have the remote execution container deployed as a worker you can switch
to remote execution. For instance when using the [Kubernetes example](../deployment-examples/kubernetes):

```shell
bazel run \
    --config=lre \
    --remote_cache=grpc://172.20.255.200:50051 \
    --strategy_regexp .*=remote \
    --remote_executor=grpc://172.20.255.201:50052 \
    //local-remote-execution/examples:hello_lre
```
