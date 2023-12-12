# Local Remote Execution

This provides rapidly upgradeable framework to construct and iterate on custom
toolchain setups that are transparent, fully hermetic and reproducible across
machines.

Nativelink's Local Remote Execution is a framework to build and rapidly iterate
custom toolchain setups that are transparent, fully hermetic and reproducible
across machines of the same system architecture.

When used in conjunction with Nix, Local Remote Execution recreates the remote
execution environment in your local development environment. This lets you
seamlessly switch between remote and local builds while reusing the same cache.

## Demo

> [!Note]
> Local Remote Execution currently only works on `x86_64-linux`.

First, create an OCI image containing with the toolchains. Make sure to run this
command *not* from within this directory, but from the root of the nativelink
repository.

```
generate-toolchains
```

The `generate-toolchains` command creates an OCI image from a nix `stdenv` and
generates toolchain configs from it. The resulting [generated C++ toolchains](
./generated/cc/BUILD) have all tools pinned to specific derivations in
`/nix/store/*`. These paths mirror the ones that you fetched when entering the
flake development environment, i.e. the tools in the container and in you local
environment are the same.

You can now build targets with the generated toolchain configs:

```
bazel run --config=lre //local-remote-execution/examples:hello_lre
```

If you have the remote execution container deployed as a remote executor you can
use switch to remote execution. For instance when using the [Kubernetes
example](../deployment-examples/kubernetes):

```
bazel run \
    --config=lre \
    --remote_cache=grpc://172.20.255.200:50051 \
    --define=EXECUTOR=remote \
    --remote_executor=grpc://172.20.255.201:50052 \
    //local-remote-execution/examples:hello_lre
```
