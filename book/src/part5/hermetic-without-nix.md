# Hermetic Toolchains Without Nix

If you don't want Nix and you don't want containers, there's a third path: download the toolchain as a build dependency and include it in the action's input tree. This is what zig-cc, `rules_rs`, `toolchains_llvm`, and similar Bazel toolchain rules do.

## The Idea

Instead of relying on the worker's installed toolchain, you fetch a prebuilt toolchain tarball by hash and make it an explicit input to every action. The compiler binary is in the Merkle tree, so it's in the action hash. Different compiler = different hash = no incorrect cache hits.

```
ActionHash = hash(Command, InputRoot, Platform)
                              │
                              └── includes /toolchain/bin/clang (fetched by hash)
```

## Available Options

NativeLink's `toolchain-examples/` directory demonstrates several:

**Source:** [`toolchain-examples/MODULE.bazel`](https://github.com/TraceMachina/nativelink/blob/main/toolchain-examples/MODULE.bazel)

### zig-cc (Hermetic C/C++ via Zig)

```python
# MODULE.bazel
bazel_dep(name = "hermetic_cc_toolchain", version = "4.0.1")

zig = use_extension("@hermetic_cc_toolchain//toolchain:ext.bzl", "toolchains")
use_repo(zig, "zig_sdk")
```

```bash
# .bazelrc
build:zig-cc --extra_toolchains @zig_sdk//toolchain:linux_amd64_gnu.2.28
```

**Source:** [`toolchain-examples/.bazelrc`](https://github.com/TraceMachina/nativelink/blob/main/toolchain-examples/.bazelrc)

Properties:
- **Hermetic:** Yes. Zig is a single statically-linked binary. No host dependencies.
- **Cross-compilation:** Excellent. Zig can target any libc version, any OS, any architecture.
- **Performance:** Optimized for binary size, not compilation speed. Slower than native LLVM.
- **Download size:** Small (~100MB).

zig-cc is the gold standard for hermetic C/C++ compilation without containers or Nix. The example [`.bazelrc`](https://github.com/TraceMachina/nativelink/blob/main/toolchain-examples/.bazelrc) registers toolchains spanning Linux (glibc 2.28/2.31 and musl, on amd64 and arm64), Windows (amd64/arm64), macOS (amd64/arm64), and `wasip1` WebAssembly — every common host and target from a single statically-linked SDK.

### toolchains_llvm (Non-Hermetic LLVM)

```python
# MODULE.bazel
bazel_dep(name = "toolchains_llvm", version = "1.3.0")

llvm = use_extension("@toolchains_llvm//toolchain/extensions:llvm.bzl", "llvm")
llvm.toolchain(llvm_version = "19.1.0")
use_repo(llvm, "llvm_toolchain")
```

```bash
# .bazelrc
build:llvm --extra_toolchains=@llvm_toolchain//:cc-toolchain-x86_64-linux
```

Properties:
- **Hermetic:** No. LLVM artifacts are dynamically linked against host glibc.
- **Cross-compilation:** Limited (requires matching sysroot).
- **Performance:** Fast. Release-optimized LLVM.
- **Download size:** Large (~1.5GB).

The non-hermeticity is the critical issue. If your worker has a different glibc than the LLVM binary expects, you get runtime link errors. If it has a compatible-but-different glibc, outputs may differ. This is why the docs comment it as non-hermetic:

```python
# - Hermetic: NO            LLVM artifacts are dynamically linked and will
#                           depend on your host's glibc.
```

Use this only when all workers are known-identical (same OS image, same packages).

### rules_rs

NativeLink provisions Rust through `rules_rs`, not mainstream `rules_rust`. `rules_rs` wraps `rules_rust` and drives toolchain download through its own module extension, fetching `rustc`, `cargo`, and the standard library by hash.

```python
# MODULE.bazel
bazel_dep(name = "rules_rs", version = "0.0.76")

rust_toolchains = use_extension("@rules_rs//rs/toolchains:module_extension.bzl", "toolchains")
rust_toolchains.toolchain(edition = "2024", version = "1.93.1")
use_repo(rust_toolchains, "default_rust_toolchains")
register_toolchains("@default_rust_toolchains//:all")
```

Properties:
- **Hermetic:** Mostly. Rust toolchain is self-contained but `rustc` may link against host libc for proc macros.
- **Cross-compilation:** Good (with proper target specification).
- **Configuration:** The `extra_exec_rustc_flags` and `extra_rustc_flags` allow per-target customization (e.g., `-Clink-self-contained=-linker` for removing host linker dependency).

### Java (Remote JDK)

```bash
# .bazelrc
build:java --java_runtime_version=remotejdk_21
build:java --tool_java_runtime_version=remotejdk_21
```

**Source:** [`toolchain-examples/.bazelrc`](https://github.com/TraceMachina/nativelink/blob/main/toolchain-examples/.bazelrc)

Java's `remotejdk` runtime versions download a JDK by hash and use it for all Java actions. This is fully hermetic (the JDK is in the input tree) and works well with remote execution.

## Testing Toolchains Against NativeLink

The repository includes a comprehensive test that builds against each toolchain variant:

**Source:** [`toolchain-examples/rbe-toolchain-test.nix`](https://github.com/TraceMachina/nativelink/blob/main/toolchain-examples/rbe-toolchain-test.nix)

This test:
1. Starts a NativeLink instance with the toolchain-examples config
2. Builds C, C++, Rust, Go, Java, and Python targets
3. Tests each toolchain configuration (`--config=zig-cc`, `--config=llvm`, `--config=java`)
4. Verifies both caching and execution

The NativeLink config for these tests:

**Source:** [`toolchain-examples/nativelink-config.json5`](https://github.com/TraceMachina/nativelink/blob/main/toolchain-examples/nativelink-config.json5)

## The Hermeticity Spectrum for Downloaded Toolchains

Not all "downloaded" toolchains are equally hermetic:

| Toolchain | Static Binary | No Host Deps | Cache Safe |
|-----------|:---:|:---:|:---:|
| zig-cc | Yes | Yes | Yes |
| rules_rs | Mostly | Mostly | Mostly |
| remotejdk | Yes | Yes | Yes |
| toolchains_llvm | No | No | Only with identical hosts |
| rules_go | Yes | Yes | Yes |
| rules_python | No | No | Only with identical hosts |

The key question: **does the toolchain binary depend on anything from the host?** If yes, the host becomes an implicit input that's not captured in the action hash.

## Combining Approaches

These approaches aren't mutually exclusive. A common production setup:

- **Nix/LRE** for the C/C++ toolchain (where hermeticity is hardest)
- **rules_rs** for Rust (self-contained enough to be hermetic without Nix)
- **Container** for integration tests (need specific system services)

Each language/action type uses whichever approach gives the best correctness/complexity tradeoff.

## Buck2 Considerations

Buck2's toolchain model is different from Bazel's. Buck2 toolchains are resolved during analysis (not execution) and are part of the rule's provider graph. The toolchain binary is typically referenced by path, not by input.

For Buck2 + hermetic toolchains without containers:
- The toolchain must be available at a known path on the worker
- The path should be captured in platform properties so the action hash changes if the toolchain changes
- Use `remote_execution_properties` in the `CommandExecutorConfig` to encode the toolchain identity

This is less automatic than Bazel's approach (where the toolchain is an explicit input) but equally correct if configured properly. See the [Buck2 chapter](../part6/buck2.md) for details.
