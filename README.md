#
<p align="center">
  <a href="https://www.nativelink.com">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/tracemachina/nativelink/main/docs/src/assets/logo-dark.svg"/>
      <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/tracemachina/nativelink/main/docs/src/assets/logo-light.svg"/>
      <img alt="NativeLink" src="https://raw.githubusercontent.com/tracemachina/nativelink/main/docs/src/assets/logo-light.svg"/>
    </picture>
  </a>
</p>

[![Homepage](https://img.shields.io/badge/Homepage-8A2BE2)](https://nativelink.com)
[![GitHub stars](https://img.shields.io/github/stars/tracemachina/nativelink?style=social)](https://github.com/TraceMachina/nativelink)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/TraceMachina/nativelink/badge)](https://securityscorecards.dev/viewer/?uri=github.com/TraceMachina/nativelink)
[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/8050/badge)](https://www.bestpractices.dev/projects/8050)
[![Slack](https://img.shields.io/badge/slack--channel-blue?logo=slack)](https://nativelink.slack.com/join/shared_invite/zt-281qk1ho0-krT7HfTUIYfQMdwflRuq7A#/shared-invite/email)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

NativeLink is an extremely (blazingly?) fast and efficient build cache and
remote executor for systems that communicate using the [Remote execution
protocol](https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto) such as [Bazel](https://bazel.build), [Buck2](https://buck2.build), and
[Reclient](https://github.com/bazelbuild/reclient). NativeLink powers several billion requests per month in production workloads and powers operating systems deployed to over one billion edge devices and hundreds of thousands of data center servers in HPC environments.

Supports Unix-based operating systems and Windows.

## 🚀 Quickstart

The setups below are **production-grade** installations. See the
[contribution docs](https://docs.nativelink.com/contribute/nix/) for
instructions on how to build from source with [Bazel](https://docs.nativelink.com/contribute/bazel/),
[Cargo](https://docs.nativelink.com/contribute/cargo/), and [Nix](https://docs.nativelink.com/contribute/nix/).

### 📦 Prebuilt images

Fast to spin up, but currently limited to `x86_64` systems. See the [container
registry](https://github.com/TraceMachina/nativelink/pkgs/container/nativelink)
for all image tags and the [contribution docs](https://docs.nativelink.com/contribute/nix)
for how to build the images yourself.

**Linux x86_64**

```bash
curl -O \
    https://raw.githubusercontent.com/TraceMachina/nativelink/main/nativelink-config/examples/basic_cas.json

# See https://github.com/TraceMachina/nativelink/pkgs/container/nativelink
# to find the latest tag
docker run \
    -v $(pwd)/basic_cas.json:/config \
    -p 50051 \
    ghcr.io/tracemachina/nativelink:v0.4.0 \
    config
```

**Windows x86_64**

```powershell
# Download the configuration file
Invoke-WebRequest `
    -Uri "https://raw.githubusercontent.com/TraceMachina/nativelink/main/nativelink-config/examples/basic_cas.json" `
    -OutFile "basic_cas.json"

# Run the Docker container
# Note: Adjust the path if the script is not run from the directory containing basic_cas.json
docker run `
    -v ${PWD}/basic_cas.json:/config `
    -p 50051 `
    ghcr.io/tracemachina/nativelink:v0.4.0 `
    config
```

### ❄️ Raw executable with Nix

Slower, since it's built from source, but more flexible and supports MacOS.
Doesn't support native Windows, but works in WSL2.

Make sure your Nix version is recent and supports flakes. For instance, install
it via the [next-gen nix installer](https://github.com/NixOS/experimental-nix-installer).

> [!CAUTION]
> Executables built for MacOS are dynamically linked against libraries from Nix
> and won't work on systems that don't have these libraries present.

**Linux, MacOS, WSL2**

```
curl -O \
    https://raw.githubusercontent.com/TraceMachina/nativelink/main/nativelink-config/examples/basic_cas.json

nix run github:TraceMachina/nativelink ./basic_cas.json
```

See the [contribution docs](https://docs.nativelink.com/contribute/nix) for
further information.

## 📜 License

Copyright 2020–2024 Trace Machina, Inc.

Licensed under the Apache 2.0 License, SPDX identifier `Apache-2.0`.
