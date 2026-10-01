# The Toolchain Problem

This is the chapter that will save you weeks. If you read nothing else in this book, read this.

The number one reason remote caching fails in practice is toolchain mismatch. Your local machine has GCC 12. CI has GCC 13. Your colleague has GCC 12 but a different libc. All three produce different object files from the same source. All three compute different action hashes. Cache hit rate: 0%.

The protocol can't help you. REAPI has no concept of "toolchain." It has platform properties (opaque strings) and command arguments (opaque bytes). The protocol assumes the client and the workers agree on what "the toolchain" is. It provides no mechanism to verify this agreement.

So you have to solve it yourself. There are three approaches, each with different tradeoffs.

## The Three Approaches

### 1. Nix / LRE (Content-Addressed Toolchains)

The toolchain is a Nix derivation. Its store path (`/nix/store/abc123-clang-17.0.1/bin/clang`) is derived from its content hash. The same source + same nixpkgs = same store path = same binary, on every machine, forever.

The platform properties include the Nix store path. Workers have the same Nix store paths available. Identity is structural — if the paths match, the toolchains are identical. If they don't, they're not.

**Correctness:** Structural (content-addressed). Bit-for-bit identical across machines.
**Cache hit rate:** Near-perfect. Same inputs + same toolchain hash = same action hash everywhere.
**Setup cost:** High. Requires Nix, requires understanding Nix, requires the LRE framework.
**Best for:** Teams that need maximum cache sharing and are willing to invest in infrastructure.

### 2. Container Images (Hash-Pinned)

The toolchain lives inside a container image. Workers pull the image and run actions inside it. The image is referenced by digest (`sha256:...`), not tag (`:latest`).

**Correctness:** By construction (image digest = fixed content). Safe if pinned by digest. Unsafe if pinned by tag (tags are mutable).
**Cache hit rate:** Good. Same image digest = same toolchain. But you must pin by digest, not tag.
**Setup cost:** Medium. Requires container runtime on workers, image registry, image build pipeline.
**Best for:** Teams with existing container infrastructure who want isolation between actions.

### 3. Hermetic Toolchains (zig-cc, rules_rust, etc.)

The toolchain is downloaded as a Bazel/Buck2 external dependency — a tarball of prebuilt binaries fetched by hash. It becomes part of the action's input tree, so it's captured in the action hash automatically.

**Correctness:** By inclusion (toolchain is in the input tree, so it's in the action hash).
**Cache hit rate:** Good for builds. Limited across the local/remote boundary (the remote worker also needs the same toolchain available in a compatible path).
**Setup cost:** Low for supported toolchains. High for custom/niche toolchains.
**Best for:** Teams that want minimal infrastructure and use standard language toolchains.

## The Fundamental Problem

All three approaches solve the same problem: **ensuring that the action hash reflects the actual toolchain that will execute the action.**

If the action hash doesn't capture the toolchain:
- Two machines with different toolchains compute the same hash
- One populates the cache
- The other gets a "cache hit" with the wrong result
- Your build is silently broken

If the action hash does capture the toolchain:
- Different toolchains → different hashes → cache misses (correct, but no sharing)
- Same toolchain → same hash → cache hits (correct and shared)

The challenge is making "same toolchain" mean what you think it means across:
- Your laptop
- Your colleague's laptop
- CI machines
- Remote execution workers
- Different operating systems and architectures

## Why Most Teams Get 5% Cache Hit Rates

The default setup for most build systems:

1. Install a toolchain on the worker host (e.g., `apt install gcc`)
2. Set `--remote_cache=grpc://...`
3. Wonder why cache hits are terrible

The problem: your local GCC and the worker's GCC are different binaries. They produce different outputs. But neither is captured in the action hash — both are just "gcc" in PATH. The action hashes look the same, so you get cache "hits" that are actually wrong, or the system detects the mismatch and gives you misses.

The fix is one of the approaches above — each a different *mechanism* for the same principle: the toolchain must be captured in the action hash. You either:
1. Content-address the toolchain (Nix)
2. Pin it by container digest
3. Include it in the action inputs

What has no fourth version is the *principle*. Everything that is not one of these mechanisms is a variant of "hope the toolchains happen to match" — which works until it doesn't, silently.

## Choosing an Approach

| Factor | Nix/LRE | Containers | Hermetic TC |
|--------|---------|------------|-------------|
| Setup complexity | High | Medium | Low |
| Correctness guarantee | Structural | By pinning | By inclusion |
| Local/remote parity | Perfect | Good (if same image) | Partial |
| Cache sharing across repos | Yes (same flake) | Yes (same image) | Only within project |
| Offline development | Yes (Nix store) | No (need pull) | Yes (after fetch) |
| Multi-language | Excellent | Good | Per-language |
| Custom toolchains | Easy (Nix derivation) | Easy (Dockerfile) | Hard (custom rules) |

The next three chapters detail each of these approaches with real configurations, real code paths, and the actual files in this repository that implement them; [Platform Properties as the Bridge](./platform-bridge.md) then shows how the platform layer wires any of them to workers. Every configuration in those chapters is real JSON5.
