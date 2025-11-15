#!/bin/bash
set -e

SOCKET="unix:///var/run/crio/crio.sock"
IMAGE="nativelink-worker-java:test"

echo "=== Testing Worker Warmup Effectiveness ==="
echo

# Test 1: Cold worker (no warmup)
echo "Test 1: Cold Worker Startup"
echo "----------------------------"
START=$(date +%s%N)

# Create pod sandbox
POD_ID=$(sudo crictl --runtime-endpoint "$SOCKET" runp <(
    cat << EOF
{
  "metadata": {
    "name": "test-pod-cold",
    "namespace": "default",
    "uid": "test-cold-$(date +%s)"
  }
}
EOF
))

# Create container
CONTAINER_ID=$(sudo crictl --runtime-endpoint "$SOCKET" create "$POD_ID" \
    <(
        cat << EOF
{
  "metadata": {
    "name": "test-container-cold"
  },
  "image": {
    "image": "$IMAGE"
  },
  "command": ["/bin/sleep", "infinity"]
}
EOF
    ) \
    <(
        cat << EOF
{
  "metadata": {
    "name": "test-pod-cold",
    "namespace": "default",
    "uid": "test-cold-$(date +%s)"
  }
}
EOF
    ))

# Start container
sudo crictl --runtime-endpoint "$SOCKET" start "$CONTAINER_ID"

# Time until Java is ready (simulate first compilation)
sudo crictl --runtime-endpoint "$SOCKET" exec "$CONTAINER_ID" \
    java -version > /dev/null 2>&1 || echo "Java check failed (expected for demo)"

END=$(date +%s%N)
COLD_TIME=$(((END - START) / 1000000))
echo "Cold worker startup time: ${COLD_TIME}ms"

# Cleanup
sudo crictl --runtime-endpoint "$SOCKET" stop "$CONTAINER_ID" 2> /dev/null || true
sudo crictl --runtime-endpoint "$SOCKET" rm "$CONTAINER_ID" 2> /dev/null || true
sudo crictl --runtime-endpoint "$SOCKET" stopp "$POD_ID" 2> /dev/null || true
sudo crictl --runtime-endpoint "$SOCKET" rmp "$POD_ID" 2> /dev/null || true

echo

# Test 2: Warm worker (with warmup)
echo "Test 2: Warm Worker Startup (with warmup)"
echo "-----------------------------------------"
START=$(date +%s%N)

# Create pod sandbox
POD_ID=$(sudo crictl --runtime-endpoint "$SOCKET" runp <(
    cat << EOF
{
  "metadata": {
    "name": "test-pod-warm",
    "namespace": "default",
    "uid": "test-warm-$(date +%s)"
  }
}
EOF
))

# Create container
CONTAINER_ID=$(sudo crictl --runtime-endpoint "$SOCKET" create "$POD_ID" \
    <(
        cat << EOF
{
  "metadata": {
    "name": "test-container-warm"
  },
  "image": {
    "image": "$IMAGE"
  },
  "command": ["/bin/sleep", "infinity"]
}
EOF
    ) \
    <(
        cat << EOF
{
  "metadata": {
    "name": "test-pod-warm",
    "namespace": "default",
    "uid": "test-warm-$(date +%s)"
  }
}
EOF
    ))

# Start container
sudo crictl --runtime-endpoint "$SOCKET" start "$CONTAINER_ID"

# Run warmup script
echo "Running warmup script..."
sudo crictl --runtime-endpoint "$SOCKET" exec "$CONTAINER_ID" \
    /opt/warmup/jvm-warmup.sh 2>&1 || echo "Warmup script not found (check image)"

# Time until Java is ready (should be much faster now)
sudo crictl --runtime-endpoint "$SOCKET" exec "$CONTAINER_ID" \
    java -version > /dev/null 2>&1 || echo "Java check failed (expected for demo)"

END=$(date +%s%N)
WARM_TIME=$(((END - START) / 1000000))
echo "Warm worker startup time: ${WARM_TIME}ms"

# Cleanup
sudo crictl --runtime-endpoint "$SOCKET" stop "$CONTAINER_ID" 2> /dev/null || true
sudo crictl --runtime-endpoint "$SOCKET" rm "$CONTAINER_ID" 2> /dev/null || true
sudo crictl --runtime-endpoint "$SOCKET" stopp "$POD_ID" 2> /dev/null || true
sudo crictl --runtime-endpoint "$SOCKET" rmp "$POD_ID" 2> /dev/null || true

echo
echo "=== Results ==="
echo "Cold worker: ${COLD_TIME}ms"
echo "Warm worker: ${WARM_TIME}ms"
if [ "$COLD_TIME" -gt 0 ]; then
    IMPROVEMENT=$(((COLD_TIME - WARM_TIME) * 100 / COLD_TIME))
    echo "Improvement: ${IMPROVEMENT}%"
fi
