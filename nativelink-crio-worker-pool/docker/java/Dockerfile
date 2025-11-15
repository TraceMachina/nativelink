FROM ubuntu:24.04

LABEL org.opencontainers.image.source="https://github.com/TraceMachina/nativelink"
LABEL org.opencontainers.image.description="NativeLink Java Worker with JVM warmup"

# Install JDK 21 and tools
RUN apt-get update && apt-get install -y --no-install-recommends \
    openjdk-21-jdk \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Set Java environment
ENV JAVA_HOME=/usr/lib/jvm/java-21-openjdk-amd64
ENV PATH="${JAVA_HOME}/bin:${PATH}"

# JVM Tuning for faster warmup and better GC
# These can be overridden via container env vars
ENV JAVA_OPTS="\
    -XX:+TieredCompilation \
    -XX:TieredStopAtLevel=1 \
    -XX:+UseG1GC \
    -XX:MaxGCPauseMillis=200 \
    -XX:+UseStringDeduplication \
    -XX:+AlwaysPreTouch \
    -XX:InitiatingHeapOccupancyPercent=45 \
    -XX:MaxRAMPercentage=75.0"

# Create warmup directory
RUN mkdir -p /opt/warmup /tmp/worker /var/log/nativelink

# Copy warmup scripts
COPY warmup/jvm-warmup.sh /opt/warmup/
COPY warmup/prime-cache.sh /opt/warmup/
COPY warmup/WarmupRunner.java /opt/warmup/
RUN chmod +x /opt/warmup/*.sh

# Compile warmup Java class
WORKDIR /opt/warmup
RUN javac WarmupRunner.java

# Install NativeLink worker binary (placeholder - should be copied from build)
# COPY nativelink-worker /usr/local/bin/
RUN printf '#!/bin/sh\necho "NativeLink worker placeholder"\nexec sleep infinity\n' > /usr/local/bin/nativelink-worker && \
    chmod +x /usr/local/bin/nativelink-worker

WORKDIR /tmp/worker

# Container will be managed by CRI-O, no entrypoint needed
CMD ["/usr/local/bin/nativelink-worker"]
