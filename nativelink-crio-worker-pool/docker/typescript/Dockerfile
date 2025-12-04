FROM node:20-alpine

LABEL org.opencontainers.image.source="https://github.com/TraceMachina/nativelink"
LABEL org.opencontainers.image.description="NativeLink TypeScript/Node.js Worker with V8 warmup"

# Install build tools and TypeScript
RUN apk add --no-cache \
    python3 \
    make \
    g++ \
    bash \
    curl \
    && npm install -g \
        typescript@5.3 \
        @bazel/bazelisk \
        esbuild \
    && rm -rf /var/cache/apk/*

# Node.js/V8 Tuning for faster warmup and better GC
# These can be overridden via container env vars
ENV NODE_OPTIONS="\
    --expose-gc \
    --max-old-space-size=4096 \
    --max-semi-space-size=128"

# Create warmup directory
RUN mkdir -p /opt/warmup /tmp/worker /var/log/nativelink

# Copy warmup scripts
COPY warmup/nodejs-warmup.sh /opt/warmup/
COPY warmup/prime-node-cache.sh /opt/warmup/
COPY warmup/warmup.js /opt/warmup/
RUN chmod +x /opt/warmup/*.sh

# Install NativeLink worker binary (placeholder - should be copied from build)
# COPY nativelink-worker /usr/local/bin/
RUN printf '#!/bin/sh\necho "NativeLink worker placeholder"\nexec sleep infinity\n' > /usr/local/bin/nativelink-worker && \
    chmod +x /usr/local/bin/nativelink-worker

WORKDIR /tmp/worker

# Container will be managed by CRI-O, no entrypoint needed
CMD ["/usr/local/bin/nativelink-worker"]
