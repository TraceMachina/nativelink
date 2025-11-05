NativeLink provides the following templates to use caching and remote execution:
- **`bazel`**:
  C++ with local remote execution using Bazel.
  Provides the same toolchain during local and remote execution to share cache
  between those builds.
  Currently restricted to Linux.
  See [Local Remote Execution](https://www.nativelink.com/docs/explanations/lre)
  for further details.

# Getting started

Install `Nix` with `flakes` enabled, for instance install it via
[experimental-nix-installer](https://github.com/NixOS/experimental-nix-installer).

Create a new directory, `cd` into it and replace `TEMPLATE_NAME` with the name
of the template to initialize your project with
```bash
nix flake init -t github:TraceMachina/nativelink#TEMPLATE_NAME
git init
git add .
```
Enter the Nix environment with `nix develop`.

Optionally install `direnv` and create `.envrc` containing
```nix
use flake
```
to automatically enter the development environment.
