# Local Remote Execution

NativeLink's Local Remote Execution is a framework to build and rapidly iterate
custom toolchain setups that are transparent, fully hermetic and reproducible
across machines of the same system architecture. When used in conjunction with
Nix, Local Remote Execution recreates the remote execution toolchain in your
local development environment. Working with LRE lets you seamlessly switch
between remote and local builds while reusing the same cache.

You must clone the repository to use Local Remote Execution. Other than that,
there are few pre-requisistes.

## Pre-Requisites

- Nix 2.19.0 or later
- A functional local container setup

## Setting up Local Remote Execution

> [!Note]
> Local Remote Execution has only been tested on `x86_64-linux`.

1. Enable a [Flake-based](https://nixos.wiki/wiki/Development_environment_with_nix-shell)
Nix Shell within the NativeLink root directory.

```bash
nix develop
```
2. Generate the LRE [OCI images](https://opencontainers.org/) containing the
toolchains:

```bash
generate-toolchains
```

You should see terminal output beginning with:

```bash
++ git rev-parse --show-toplevel
```

The [`generate-toolchains`](../tools/generate-toolchains.nix) command creates
OCI images from the [lre-cc](./lre-cc.nix) and [lre-java](./lre-java.nix) images
and generates toolchain configurations from it. The resulting generated [C++ toolchains](./generated/cc/BUILD)
and [Java toolchains](./generated-java/java/BUILD) have their dependencies
pinned to specific derivations in `/nix/store/*`. These paths mirror the ones
that you fetched when entering the flake development environment, that is, the
tools in the container and in your local environment are the same.

3. You can now build targets with the generated toolchain configurations. To
build the `hello_lre` target in the [examples](./examples/) directory:

```bash
bazel run --config=lre @local-remote-execution//examples:hello_lre
```

🎉 That's it! You've built a target with Local Remote Execution using NativeLink. 🎉

## Switching to Remote Execution

If you have the remote execution container deployed as a worker you can switch
to remote execution. For instance when using the [Kubernetes example](../deployment-examples/kubernetes):

```bash
bazel run \
    --config=lre \
    --remote_cache=grpc://172.20.255.200:50051 \
    --strategy_regexp .*=remote \
    --remote_executor=grpc://172.20.255.201:50052 \
    @local-remote-execution//examples:hello_lre
```

## Architecture

The original C++ and Java toolchain containers are never really instantiated.
Instead, their container environments are used and passed through transforming
functions that take a container schematic as input and generate some other
output.

The first transform is the [`createWorker`](../tools/create-worker.nix) wrapper
which converts an arbitrary OCI image to a NativeLink worker:

```mermaid
flowchart LR
    A(some-toolchain)
        --> |wrap with nativelink| B(nativelink-worker-some-toolchain)
        --> |deploy| K{Kubernetes}
```

In the case of LRE the base image is built with Nix for perfect reproducibility.
However, you could use a more classic toolchain container like an Ubuntu base
image as well:

```mermaid
flowchart LR
    A(lre-cc)
        --> |wrap with nativelink| B(nativelink-worker-lre-cc)
        --> |deploy| K{Kubernetes}
    C(lre-java)
        --> |wrap with nativelink| D(nativelink-worker-lre-java)
        --> |deploy| K{Kubernetes}
```

The second transform is some mechanism that generates toolchain configurations
for your respective RBE client:

```mermaid
flowchart LR
    A(some-toolchain)
        --> |custom toolchain generation logic| B[RBE client configuration]
```

In many classic setups the RBE client configurations are handwritten. In the
case of LRE we're generating Bazel toolchains and platforms using a pipeline of
custom image generators and the `rbe_configs_gen` tool:

```mermaid
flowchart LR
    A(lre-cc)
        --> |wrap with autogen tooling| B(rbe-autogen-lre-cc)
        --> |rbe_configs_gen| C[Bazel C++ toolchain and platform]
```

When you then invoke your RBE client with the configuration set up to use these
toolchains, the NativeLink scheduler matches actions to the worker they require.

In Bazel's case this scheduler endpoint is set via the `--remote_executor` flag.

## Isn't this approach generic?

Yes. The general approach described works for arbitrary toolchain containers.
You might need to implement your own logic to get from the toolchain container
to some usable RBE configuration files (or write them manually), but this can be
considered an implementation detail specific to your requirements.

LRE goes one step further in that it lets you reproduce the remote execution
environment via a Nix's `devShell` locally. This let's you seamlessly switch
between local and remote execution and reuse caches between local and
remote-enabled builds.

The LRE approach is most useful when toolchain requirements become increasingly
complex and require the flexibility to rapidly iterate. Example use-cases which
strongly influenced the LRE design are toolchains involving GPUs (CUDA, HIP etc)
and projects where the toolchain itself is part of first party code, for
instance [LLVM](https://github.com/llvm/llvm-project).
