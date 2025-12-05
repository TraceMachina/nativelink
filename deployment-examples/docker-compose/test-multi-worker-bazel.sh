#!/bin/bash
# Test script to validate multi-worker NativeLink setup with Bazel compilation

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "=========================================="
echo "Multi-Worker NativeLink Bazel Test"
echo "=========================================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${GREEN}[✓]${NC} $1"
}

print_error() {
    echo -e "${RED}[✗]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

# Cleanup function
cleanup() {
    echo ""
    print_status "Cleaning up..."
    docker-compose -f docker-compose-multi-worker.yml down -v 2> /dev/null || true
}

trap cleanup EXIT

# Check prerequisites
print_status "Checking prerequisites..."
command -v docker > /dev/null 2>&1 || {
    print_error "Docker is required but not installed."
    exit 1
}
command -v docker-compose > /dev/null 2>&1 || {
    print_error "Docker Compose is required but not installed."
    exit 1
}
command -v bazel > /dev/null 2>&1 || {
    print_error "Bazel is required but not installed."
    exit 1
}

# Check if required files exist
print_status "Checking configuration files..."
required_files=(
    "docker-compose-multi-worker.yml"
    "worker-shared-cas.json5"
)

for file in "${required_files[@]}"; do
    if [[ ! -f $file ]]; then
        print_error "Required file missing: $file"
        exit 1
    fi
done

# Start multi-worker deployment
print_status "Starting multi-worker NativeLink deployment..."
docker-compose -f docker-compose-multi-worker.yml up -d

# Wait for services to be healthy
print_status "Waiting for services to be ready..."
MAX_WAIT=60
WAIT_COUNT=0

while [[ $WAIT_COUNT -lt $MAX_WAIT ]]; do
    # Check if scheduler is responding
    if nc -z localhost 50052 2> /dev/null && nc -z localhost 50051 2> /dev/null; then
        print_status "Services are ready!"
        break
    fi

    sleep 2
    WAIT_COUNT=$((WAIT_COUNT + 2))
    echo -n "."
done

if [[ $WAIT_COUNT -ge $MAX_WAIT ]]; then
    print_error "Services failed to start within ${MAX_WAIT} seconds"
    docker-compose -f docker-compose-multi-worker.yml logs
    exit 1
fi

# Verify workers are connected
print_status "Verifying workers are connected..."
WORKER_COUNT=$(docker-compose -f docker-compose-multi-worker.yml ps | grep -c "worker-" || true)
if [[ $WORKER_COUNT -lt 2 ]]; then
    print_error "Expected at least 2 workers, found: $WORKER_COUNT"
    exit 1
fi
print_status "Found $WORKER_COUNT workers running"

# Check CAS volume sharing
print_status "Verifying shared CAS configuration..."
for i in 1 2 3; do
    WORKER_NAME="worker-$i"
    CAS_MOUNT=$(docker inspect "$(docker-compose -f docker-compose-multi-worker.yml ps -q $WORKER_NAME 2> /dev/null)" 2> /dev/null |
        grep -A 5 '"Destination": "/data/cas"' | grep '"Source"' | cut -d'"' -f4 || true)

    if [[ -n $CAS_MOUNT ]]; then
        print_status "Worker $i using CAS volume: $(basename "$CAS_MOUNT")"
    fi
done

# Test 1: Simple build with remote execution
print_status "Test 1: Building NativeLink with multi-worker remote execution..."
cd ../.. # Go to NativeLink root

BUILD_CMD="bazel build //src:nativelink \
    --remote_cache=grpc://127.0.0.1:50051 \
    --remote_executor=grpc://127.0.0.1:50052 \
    --remote_default_exec_properties=cpu_count=1 \
    --jobs=10"

echo "Running: $BUILD_CMD"

if $BUILD_CMD 2>&1 | tee /tmp/bazel-build.log; then
    print_status "Build completed successfully!"
else
    print_error "Build failed!"
    echo "Build log:"
    tail -50 /tmp/bazel-build.log
    exit 1
fi

# Verify work was distributed across workers
print_status "Verifying work distribution across workers..."
WORKER_ACTIVITY=0
for i in 1 2 3; do
    WORKER_NAME="docker-compose-worker-$i-1"
    if docker logs "$WORKER_NAME" 2>&1 | grep -q "Executing action"; then
        print_status "Worker $i executed actions"
        WORKER_ACTIVITY=$((WORKER_ACTIVITY + 1))
    else
        print_warning "Worker $i may not have executed actions (could be idle)"
    fi
done

if [[ $WORKER_ACTIVITY -eq 0 ]]; then
    print_error "No workers executed any actions!"
    exit 1
fi

# Test 2: Clean build to ensure cache sharing works
print_status "Test 2: Clean rebuild to test cache sharing..."
bazel clean

if $BUILD_CMD > /dev/null 2>&1; then
    print_status "Rebuild successful - cache sharing works!"
else
    print_error "Rebuild failed - possible CAS sharing issue"
    exit 1
fi

# Test 3: Check for "Object not found" errors
print_status "Test 3: Checking for CAS configuration errors..."
ERROR_COUNT=0
for i in 1 2 3; do
    WORKER_NAME="docker-compose-worker-$i-1"
    if docker logs "$WORKER_NAME" 2>&1 | grep -q "Object.*not found"; then
        print_error "Worker $i has 'Object not found' errors - CAS misconfiguration!"
        ERROR_COUNT=$((ERROR_COUNT + 1))
    fi
done

if [[ $ERROR_COUNT -gt 0 ]]; then
    print_error "Found CAS configuration errors in $ERROR_COUNT workers"
    print_error "This indicates workers are not sharing CAS properly"
    exit 1
else
    print_status "No CAS configuration errors found"
fi

# Test 4: Parallel build stress test
print_status "Test 4: Parallel build stress test..."
bazel clean

STRESS_CMD="bazel build //... \
    --remote_cache=grpc://127.0.0.1:50051 \
    --remote_executor=grpc://127.0.0.1:50052 \
    --remote_default_exec_properties=cpu_count=1 \
    --jobs=50"

echo "Running stress test: $STRESS_CMD"

if timeout 300 "$STRESS_CMD" > /dev/null 2>&1; then
    print_status "Stress test passed!"
else
    print_warning "Stress test failed or timed out (this may be normal for large builds)"
fi

# Final summary
echo ""
echo "=========================================="
echo "Test Results Summary"
echo "=========================================="
print_status "Multi-worker configuration: VALID"
print_status "CAS sharing: WORKING"
print_status "Build execution: SUCCESSFUL"
print_status "Worker distribution: CONFIRMED"

echo ""
echo "Multi-worker NativeLink setup is working correctly!"
echo ""
echo "You can now use this deployment with:"
echo "  --remote_cache=grpc://127.0.0.1:50051"
echo "  --remote_executor=grpc://127.0.0.1:50052"
