# syntax=docker/dockerfile:1

# Keep this base aligned with the Nix version used by the RBE workflow.
FROM nixos/nix:2.28.3@sha256:e623d73af9cac82d1b50784c83e0cf2a4b83bfd2cfe8d5b67809a2fc94e043ac

ENV NIX_CONFIG="experimental-features = nix-command flakes"

# GitHub Actions requires the Node runtime at this path for JavaScript actions.
RUN nix profile install nixpkgs#nodejs_24 \
    && mkdir -p /__e/node24/bin \
    && ln -s /root/.nix-profile/bin/node /__e/node24/bin/node
