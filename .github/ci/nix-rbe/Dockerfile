# syntax=docker/dockerfile:1

# Keep this base aligned with the Nix version used by the RBE workflow.
FROM nixos/nix:2.28.3@sha256:e623d73af9cac82d1b50784c83e0cf2a4b83bfd2cfe8d5b67809a2fc94e043ac

ENV NIX_CONFIG="experimental-features = nix-command flakes"

# The runner bind-mounts its own glibc Node over /__e, so a symlink placed
# there is hidden. That binary's interpreter is /lib64/ld-linux-x86-64.so.2.
# Point the interpreter at Nix glibc and put libstdc++ where that loader looks.
RUN set -eu \
    && nix profile install nixpkgs#nodejs_24 \
    && mkdir -p /__e/node24/bin /lib64 \
    && ln -s /root/.nix-profile/bin/node /__e/node24/bin/node \
    && node=$(readlink -f /root/.nix-profile/bin/node) \
    && loader=$(grep -a -o '/nix/store/[^ ]*/ld-linux-x86-64.so.2' "$node" | head -1) \
    && ln -s "$loader" /lib64/ld-linux-x86-64.so.2 \
    && glibc=$(dirname "$loader") \
    && stdcxx=$(nix-store -qR "$node" | while read -r path; do \
         if [ -e "$path/lib/libstdc++.so.6" ]; then printf '%s\n' "$path/lib/libstdc++.so.6"; break; fi; \
       done) \
    && ln -s "$stdcxx" "$glibc/libstdc++.so.6" \
    && ln -s "$(dirname "$stdcxx")/libgcc_s.so.1" "$glibc/libgcc_s.so.1" \
    && ln -s "$(find /nix/store -path '*-zlib-*/lib/libz.so.1' | head -1)" "$glibc/libz.so.1"

# The runner checks the workspace out as uid 1001 but the job runs as root,
# so libgit2 refuses to open it and every flake fetch fails. The system file
# is the one Nix reads: HOME points at /github/home, which root does not own.
RUN printf '[safe]\n\tdirectory = *\n' > /etc/gitconfig

# nixos/nix leaves /bin/sh as the only entry in /bin, but scripts the tests
# run, bazel-retry among them, carry a /bin/bash shebang.
RUN ln -s "$(readlink -f /bin/sh)" /bin/bash

# nixos/nix ships max-jobs = 1, so anything missing from the store builds
# serially. The Determinate installer this image replaces leaves it at auto.
RUN printf '%s\n' \
    'max-jobs = auto' \
    'fallback = true' \
    'keep-outputs = true' \
    'log-lines = 100' \
    >> /etc/nix/nix.conf

# Warm the store and the fetcher cache with everything an RBE job pulls before
# its first test. The fetcher cache lives under HOME rather than in the store,
# so the job has to run with HOME=/root to get the warm copy.
#
# TraceMachina/attic is deliberately absent: the nativelink attic cache serves
# it, and this build cannot reach that cache, so it would compile from source.
COPY flake.nix flake.lock /opt/warm-flake/
RUN set -eu \
    && rev=$(nix eval --raw --impure --expr \
         '(builtins.fromJSON (builtins.readFile /opt/warm-flake/flake.lock)).nodes.nixpkgs.locked.rev') \
    && printf '{"version":2,"flakes":[{"from":{"type":"indirect","id":"nixpkgs"},"to":{"type":"github","owner":"NixOS","repo":"nixpkgs","rev":"%s"}}]}' \
         "$rev" > /etc/nix/registry.json \
    && cd /opt/warm-flake \
    && nix flake archive --json > /dev/null \
    && nix build --no-link nixpkgs#attic-client \
    && nix-store --optimise
