FROM swift:6.0-jammy

LABEL org.opencontainers.image.source="https://github.com/TraceMachina/nativelink"
LABEL org.opencontainers.image.description="NativeLink Swift Worker with swiftc warmup"

# Build essentials and curl for cache priming
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    git \
    && rm -rf /var/lib/apt/lists/*

# Swift compiler tuning. Set via env so deployments can override per-pool.
#   SWIFT_DETERMINISTIC_HASHING=1 keeps swiftc output deterministic when
#     multiple jobs hit the same template (helps caches dedupe).
#   SWIFTC_FLAGS is consumed by the warmup script and by deployments that
#     want to invoke swiftc through a wrapper.
ENV SWIFT_DETERMINISTIC_HASHING=1
ENV SWIFTC_FLAGS="-O"
# The module cache is the on-disk cost we want preserved across jobs via
# the COW lower layer. Keep it under /opt/warmup so the template captures
# it and the per-job upper layer never needs to repopulate it.
ENV SWIFT_MODULE_CACHE=/opt/warmup/module-cache

# Create warmup + worker directories
RUN mkdir -p /opt/warmup /tmp/worker /var/log/nativelink "${SWIFT_MODULE_CACHE}"

# Copy warmup payload
COPY warmup/swift-warmup.sh /opt/warmup/
COPY warmup/prime-swift-cache.sh /opt/warmup/
COPY warmup/SwiftWarmup.swift /opt/warmup/
RUN chmod +x /opt/warmup/*.sh

# Compile the warmup binary once at image-build time so the on-disk
# warmup runs at template-creation time, not on every job.
WORKDIR /opt/warmup
RUN swiftc ${SWIFTC_FLAGS} \
        -module-cache-path "${SWIFT_MODULE_CACHE}" \
        -o /opt/warmup/SwiftWarmup \
        SwiftWarmup.swift && \
    /opt/warmup/SwiftWarmup 100

# NativeLink worker binary placeholder - real deployments mount or COPY
# the actual binary in. Same convention as the Java/TypeScript images.
RUN printf '#!/bin/sh\necho "NativeLink worker placeholder"\nexec sleep infinity\n' > /usr/local/bin/nativelink-worker && \
    chmod +x /usr/local/bin/nativelink-worker

WORKDIR /tmp/worker

# CRI-O drives lifecycle; no entrypoint needed.
CMD ["/usr/local/bin/nativelink-worker"]
