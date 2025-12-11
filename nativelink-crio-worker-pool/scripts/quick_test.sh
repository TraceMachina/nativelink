#!/bin/bash
# Quick smoke test to verify CRI-O and worker images are working

set -e

SOCKET="unix:///var/run/crio/crio.sock"

echo "=== Quick Test: CRI-O Warm Workers ==="
echo

# Check CRI-O is running
echo "1. Checking CRI-O..."
if ! sudo crictl --runtime-endpoint "$SOCKET" version > /dev/null 2>&1; then
    echo "❌ CRI-O is not running!"
    echo "   Start it with: sudo systemctl start crio"
    exit 1
fi
echo "✓ CRI-O is running"

# Check for images
echo
echo "2. Checking for worker images..."
if sudo crictl --runtime-endpoint "$SOCKET" images | grep -q "nativelink-worker-java"; then
    echo "✓ Java worker image found"
else
    echo "⚠ Java worker image not found"
    echo "  Build it with: docker build -t nativelink-worker-java:test docker/java/"
fi

if sudo crictl --runtime-endpoint "$SOCKET" images | grep -q "nativelink-worker-node"; then
    echo "✓ Node worker image found"
else
    echo "⚠ Node worker image not found"
    echo "  Build it with: docker build -t nativelink-worker-node:test docker/typescript/"
fi

# Try to create a simple container
echo
echo "3. Testing container creation..."
POD_ID=$(sudo crictl --runtime-endpoint "$SOCKET" runp <(
    cat << EOF
{
  "metadata": {
    "name": "quick-test-pod",
    "namespace": "default",
    "uid": "quick-test-$(date +%s)"
  }
}
EOF
))
echo "✓ Pod sandbox created: $POD_ID"

# Use a simple alpine image for quick test
CONTAINER_ID=$(sudo crictl --runtime-endpoint "$SOCKET" create "$POD_ID" \
    <(
        cat << EOF
{
  "metadata": {
    "name": "quick-test-container"
  },
  "image": {
    "image": "alpine:latest"
  },
  "command": ["sh", "-c", "echo 'Hello from CRI-O!' && sleep 5"]
}
EOF
    ) \
    <(
        cat << EOF
{
  "metadata": {
    "name": "quick-test-pod",
    "namespace": "default",
    "uid": "quick-test-$(date +%s)"
  }
}
EOF
    ))
echo "✓ Container created: $CONTAINER_ID"

sudo crictl --runtime-endpoint "$SOCKET" start "$CONTAINER_ID"
echo "✓ Container started"

# Wait and check logs
sleep 2
echo
echo "Container output:"
sudo crictl --runtime-endpoint "$SOCKET" logs "$CONTAINER_ID" || echo "(no logs yet)"

# Cleanup
echo
echo "4. Cleaning up..."
sudo crictl --runtime-endpoint "$SOCKET" stop "$CONTAINER_ID" 2> /dev/null || true
sudo crictl --runtime-endpoint "$SOCKET" rm "$CONTAINER_ID" 2> /dev/null || true
sudo crictl --runtime-endpoint "$SOCKET" stopp "$POD_ID" 2> /dev/null || true
sudo crictl --runtime-endpoint "$SOCKET" rmp "$POD_ID" 2> /dev/null || true
echo "✓ Cleanup complete"

echo
echo "=== ✅ Quick test passed! ==="
echo
echo "Next steps:"
echo "  - Build worker images if you haven't"
echo "  - Run full warmup test: ./scripts/test_warmup.sh"
echo "  - Run benchmark suite: ./scripts/benchmark_all.sh"
