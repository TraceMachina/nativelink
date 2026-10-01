# Nix and LRE

Local Remote Execution (LRE) is NativeLink's answer to the toolchain problem. It uses Nix to create toolchains that are identical — bit-for-bit — on your laptop, in CI, and on remote workers. This gives you structural cache correctness and near-perfect hit rates.

LRE is the most powerful approach to toolchains in the remote execution world. It is also the most complex to set up. This chapter explains both.

## The Idea

Nix is a package manager where every package is identified by a store path derived from its build inputs:

```
/nix/store/h590aa9zij6n1kimlnj9fyh038rmj396-clang-wrapper-22.1.8/bin/clang
```

That path is a hash of: the Clang source, the build flags, the compiler used to compile Clang, the libc, and all transitive dependencies. If any of these change, the hash changes, and you get a different path. (The exact hashes and versions above are the ones the current LRE C++ toolchain reports — Clang `22.1.8`, glibc `2.42`, libc++ `22.1.8` — recorded in [`local-remote-execution/generated-cc/cc/builtin_include_directory_paths`](https://github.com/TraceMachina/nativelink/blob/main/local-remote-execution/generated-cc/cc/builtin_include_directory_paths#L7).)

This means:
- Same source + same nixpkgs revision = same store path = same binary, everywhere
- Different source or different nixpkgs = different store path = different binary, unambiguously

LRE exploits this property: if your local development environment and the remote worker both resolve the toolchain to `/nix/store/h590aa9...clang-wrapper-22.1.8/bin/clang`, they have byte-identical compilers. Actions that use this compiler produce byte-identical outputs. Cache hits are guaranteed.

The subtlety — and the part most write-ups get wrong — is *how* that identity reaches the scheduler. Workers do **not** advertise the store path as a platform-property value. The toolchain closure is packaged into an OCI image, and the image's content-addressed tag (`docker://lre-cc:<nix-hash>`) travels as the `container-image` platform property. The store paths live *inside* that image; the tag is their fingerprint. [Step 5](#step-5-worker-configuration) walks through the real properties.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Your Nix flake (defines toolchain derivations)             │
├─────────────────────────────────────────────────────────────┤
│  LRE Flake Module (generates lre.bazelrc, platform config)  │
├─────────────────────────────────────────────────────────────┤
│  Worker Container Image (Nix closure with toolchain)        │
├─────────────────────────────────────────────────────────────┤
│  Platform Properties (`container-image` tag = toolchain id) │
└─────────────────────────────────────────────────────────────┘
```

The flow:
1. A Nix flake defines your toolchains (C/C++, Rust, etc.)
2. The LRE flake module generates Bazel configuration that selects the LRE toolchains and execution platform
3. A container image is built from the Nix closure (containing the toolchain binaries)
4. Workers run this container image
5. The Bazel execution platform stamps a `container-image` exec property whose tag embeds the Nix hash of that closure
6. Client and worker agree on toolchain identity through that content-addressed tag — not by exchanging bare store paths

## Setting Up LRE

**Source:** [`local-remote-execution/`](https://github.com/TraceMachina/nativelink/tree/main/local-remote-execution)

### Step 1: Flake Inputs

Your project's `flake.nix` imports NativeLink's flake and follows its nixpkgs:

```nix
# flake.nix
{
  inputs = {
    nixpkgs = {
      url = "github:nixos/nixpkgs";
      # CRITICAL: follows nativelink's nixpkgs so store paths match
      follows = "nativelink/nixpkgs";
    };
    nativelink.url = "github:TraceMachina/nativelink/<commit>";
  };
}
```

The `follows` is essential. If your local nixpkgs and the worker's nixpkgs differ, Nix store paths will differ (even for the same package version), and you'll get cache misses.

**Source:** [`local-remote-execution/README.md`](https://github.com/TraceMachina/nativelink/blob/main/local-remote-execution/README.md)

### Step 2: LRE Flake Module

Import the flake module and generate `lre.bazelrc` in your shell hook:

```nix
# flake.nix (continued)
{
  imports = [ inputs.nativelink.flakeModule ];

  devShells.default = pkgs.mkShell {
    shellHook = ''
      ${config.lre.installationScript}
    '';
  };
}
```

When you enter the dev shell (via `direnv` or `nix develop`), it generates:

```bash
# lre.bazelrc (auto-generated, x86_64-linux host shown)
build --define=EXECUTOR=remote
build --extra_execution_platforms=@local-remote-execution//rust/platforms:x86_64-unknown-linux-gnu,@local-remote-execution//rust/platforms:x86_64-unknown-linux-musl,@local-remote-execution//generated-cc/config:platform
build --action_env=BAZEL_DO_NOT_DETECT_CPP_TOOLCHAIN=1
build --extra_toolchains=@local-remote-execution//generated-cc/config:cc-toolchain
build --extra_toolchains=@local-remote-execution//rust:rust-x86_64-linux
build --extra_toolchains=@local-remote-execution//rust:rustfmt-x86_64-linux
build --platforms=@local-remote-execution//rust/platforms:x86_64-unknown-linux-musl
```

The Rust toolchains and platforms are emitted for every supported host; the C++ block (`--extra_toolchains=...generated-cc/config:cc-toolchain` and the `generated-cc/config:platform` execution platform) is added only on Linux, because `lre-cc` is `x86_64-linux`-only. Setting `lre.prefix` gates the whole set behind `--config=<prefix>` instead of enabling it by default.

**Source:** [`local-remote-execution/flake-module.nix:139-191`](https://github.com/TraceMachina/nativelink/blob/main/local-remote-execution/flake-module.nix#L139-L191)

### Step 3: Generated Toolchain Configuration

The LRE module generates Bazel BUILD files describing the C++ toolchain and its execution platform. The `cc-compiler` targets under `generated-cc/cc/` point at the wrapped Clang in the Nix store; the execution `platform` below carries the identity forward as an exec property:

```python
# local-remote-execution/generated-cc/config/BUILD (auto-generated)
platform(
    name = "platform",
    constraint_values = [
        "@platforms//os:linux",
        "@platforms//cpu:x86_64",
        "@bazel_tools//tools/cpp:clang",
    ],
    exec_properties = {
        "container-image": "docker://lre-cc:zms5771rx1yqb4wd6qbj5f9sb2paq75k",
        "OSFamily": "Linux",
    },
    parents = ["@platforms//host"],
)
```

**Source:** [`local-remote-execution/generated-cc/config/BUILD:35-47`](https://github.com/TraceMachina/nativelink/blob/main/local-remote-execution/generated-cc/config/BUILD#L35-L47)

This is where the C++ toolchain identity actually lives: the `container-image` exec property. Its tag includes the Nix hash of the `lre-cc` closure — if the toolchain derivation changes, the tag changes, the platform changes, and every action hash that runs on it changes. Cache correctness is structural. Note that no store path appears here; the paths are sealed inside the image the tag names.

### Step 4: Worker Container Image

The LRE container image is built from the Nix closure:

**Source:** [`local-remote-execution/overlays/lre-cc.nix`](https://github.com/TraceMachina/nativelink/blob/main/local-remote-execution/overlays/lre-cc.nix)

This produces a minimal OCI image containing only the Nix store paths needed by the toolchain — no full OS, no package manager, only the toolchain binaries and libraries the build requires.

### Step 5: Worker Configuration

Here is where the fabricated version of this story usually appears: "the worker advertises `lre-cc` and `lre-rs` set to Nix store paths." It does not. A real LRE worker advertises the properties as *labels*, and the values are mostly empty strings — the identity rides elsewhere. From the shared-CAS worker example:

```json5
// deployment-examples/docker-compose/worker-shared-cas.json5:63-88
platform_properties: {
  cpu_count: { query_cmd: "nproc" },
  OSFamily: { values: [""] },
  "container-image": { values: [""] },
  "lre-rs": { values: [""] },
  ISA: { values: ["x86-64"] },
}
```

Two things stand out. First, there is no `lre-cc` property at all — the C++ toolchain identity is not a worker label; it rides in the `container-image` tag stamped by the Bazel platform (`docker://lre-cc:<nix-hash>`, from [Step 3](#step-3-generated-toolchain-configuration)). Second, `lre-rs` and `container-image` carry empty-string values: they are advertised so jobs *may* request them, but the worker does not pin them to a path.

Whether an advertised property restricts scheduling is decided by the scheduler, not the worker. The `simple` scheduler classifies each property:

```json5
// deployment-examples/docker-compose/scheduler.json5:36-44
supported_platform_properties: {
  cpu_count: "minimum",
  OSFamily: "priority",
  "container-image": "priority",
  "lre-rs": "priority",
  ISA: "exact",
}
```

The classifier is defined in [`nativelink-config/src/schedulers.rs:43-65`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/src/schedulers.rs#L43-L65):

- `minimum` — numeric floor; the worker must report at least the requested value (`cpu_count`).
- `exact` — string equality; the worker's value must match the job's exactly (`ISA`).
- `priority` — **does not restrict** matching. The value is passed to the worker as an informational field (and, in future, will bias worker preference). Both `container-image` and `lre-rs` are `priority`.

The toolchain tag therefore never acts as a hard filter — the worker running the matching `lre-cc` image *is* the correct executor, and the tag rides through as content-addressed identity that folds into the action hash. `ISA: "exact"` and `cpu_count: "minimum"` do the actual filtering. This is why the empty-string worker values are harmless: `priority` properties are informational, not constraints.

## Why This Works

The chain of content-addressing:

1. Nix derivation inputs → Nix store paths (hash of inputs)
2. Store path closure → `lre-cc` OCI image → `container-image` tag (embeds the Nix hash)
3. `container-image` tag → Bazel execution-platform exec property (`generated-cc/config/BUILD:42-45`)
4. Exec property → the `Platform` in the REv2 `Action` proto → the action digest the scheduler and CAS key on

If the toolchain changes at step 1, everything downstream changes. If it doesn't change, everything stays the same. There's no ambiguity, no "did someone update the tag?", no "is the worker running the right version?"

## The Rust Toolchain

LRE includes pre-built Rust toolchain configurations:

**Source:** [`local-remote-execution/rust/BUILD.bazel`](https://github.com/TraceMachina/nativelink/blob/main/local-remote-execution/rust/BUILD.bazel)

These define `rust_toolchain` targets pointing at Nix store paths for `rustc`, `cargo`, `clippy`, `rustfmt`, and the standard library. Same principle: paths are content-addressed, identity is structural.

Unlike `lre-cc`, the Rust toolchains (`lre-rs`) are multi-platform: the flake module emits `rust`/`rustfmt` toolchains and `rust/platforms` targets for `aarch64-linux`, `x86_64-linux`, `aarch64-darwin`, and `x86_64-darwin` ([`flake-module.nix:98-120,177-191`](https://github.com/TraceMachina/nativelink/blob/main/local-remote-execution/flake-module.nix#L98-L120)). C++ is the laggard here, not Rust.

## Limitations

1. **`lre-cc` is `x86_64-linux` only.** The C++ execution platform and `cc-toolchain` are emitted only on Linux, and only for `x86_64` (`flake-module.nix:164-175`; the `TODO` there tracks broadening it). Rust (`lre-rs`) already spans `aarch64`/`x86_64` on Linux and macOS — so a mixed repo can get portable Rust LRE today while C++ stays x86_64-linux.
2. **Requires Nix on the toolchain path.** Building the closures and the `lre-cc` image needs Nix; every worker also needs Nix or the pre-pulled image. Authoring the toolchain is a Nix task.
3. **nixpkgs alignment is critical.** If local and remote nixpkgs diverge, store paths diverge, and you lose cache sharing. The `follows` in flake.nix is not optional.
4. **First-time setup is complex.** The Nix ecosystem has a steep learning curve. Once set up, maintenance is low (update the flake input, regenerate).
5. **Bazel-focused.** LRE's toolchain generation currently targets Bazel. Buck2 integration requires manual platform configuration (see Part VI).

## When to Use LRE

Use LRE when:
- Cache hit rates matter (large team, expensive builds)
- You need local/CI/remote parity (debugging remote failures locally)
- You're willing to invest in Nix infrastructure
- You want structural correctness, not correctness-by-convention

Don't use LRE when:
- Your team is small and all on the same machine type
- You can't justify the Nix learning curve
- You only need remote caching (not execution)
- Your builds are already fast enough without cache sharing
