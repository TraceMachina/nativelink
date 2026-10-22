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
    && ln -s "$(dirname "$stdcxx")/libgcc_s.so.1" "$glibc/libgcc_s.so.1"

# The runner checks the workspace out as uid 1001 but the job runs as root,
# so libgit2 refuses to open it and every flake fetch fails. The system file
# is the one Nix reads: HOME points at /github/home, which root does not own.
RUN printf '[safe]\n\tdirectory = *\n' > /etc/gitconfig
