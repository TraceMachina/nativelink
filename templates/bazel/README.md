# Getting started

Get your credentials and paste them into `user.bazelrc`
```
build --remote_cache=grpcs://TODO
build --bes_backend=grpcs://TODO
build --remote_timeout=600
build --remote_executor=grpcs://TODO
```

You're ready to build the provided example with `bazel build hello-world`.

# Build configuration

- **`flake.nix`**:
  Add development tools and build dependencies under `packages`.
  If you add build dependencies, add them to the RBE image and update the
  image URL.
  If you change the NativeLink input to another git hash, adjust the URL of
  the LRE Bazel module in `MODULE.bazel`, the URL to the container image in
  `platforms/BUILD.bazel`, and update the flake inputs with
  `nix flake update`.
  Upon changes, don't forget to re-enter the Nix environment with
  `nix develop`.

- **`user.bazelrc`**:
  Add Bazel flags to your builds, see
  [Command-Line Reference](https://bazel.build/reference/command-line-reference).
  Don't forget to add your NativeLink cloud credentials or set `remote_cache`
  and `remote_executor` to your on-prem solution, see
  [remote execution infrastructure](https://www.nativelink.com/docs/rbe/remote-execution-examples#preparing-the-remote-execution-infrastructure).

- **`BUILD.bazel`**:
  The top level Bazel build file, see
  [C/C++ Rules](https://bazel.build/reference/be/c-cpp).

- **`platforms/BUILD.bazel`**:
  The platform `lre-cc` specifies the URL of the `container-image` that gets
  passed to the NativeLink cloud with `exec_properties`.
  This platform inherits its properties from the LRE Bazel module.

# Code quality and CI

- **`pre-commit-hooks.nix`**:
  Configure linting and formatting for CI, see
  [available hooks](https://devenv.sh/reference/options/#pre-commithooks).
  You can manually call them with `pre-commit run -a`.

- **`.github/workflows`**:
  Contains pre-configured GitHub workflows for pre-commit hooks and building
  your project.
