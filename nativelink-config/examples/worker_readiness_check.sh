#!/bin/bash
# Example readiness check script for NativeLink workers
#
# This script is executed before the worker registers with the scheduler.
# Exit 0 = ready, non-zero = not ready (will retry)
#
# Configure in worker config:
#   "readiness_check_script": "/path/to/worker_readiness_check.sh"
#   "readiness_timeout_secs": 600
#
# Common checks to perform:
# 1. Docker daemon availability (for container-based execution)
# 2. Disk space in work directory
# 3. Network connectivity to CAS
# 4. GPU driver availability (for GPU workers)

set -e

# Configuration (adjust these values for your environment)
MIN_DISK_GB=${MIN_DISK_GB:-10}
CAS_HOST=${CAS_HOST:-"localhost"}
CAS_PORT=${CAS_PORT:-50051}
WORK_DIR=${WORK_DIR:-"/tmp/nativelink/work"}

echo "Running worker readiness checks..."

# Check 1: Docker daemon (if using container execution)
if command -v docker &> /dev/null; then
    echo "Checking Docker daemon..."
    if ! docker info &> /dev/null; then
        echo "ERROR: Docker daemon not ready"
        exit 1
    fi
    echo "  Docker daemon: OK"
fi

# Check 2: Disk space in work directory
echo "Checking disk space..."
if [ -d "$WORK_DIR" ] || mkdir -p "$WORK_DIR" 2> /dev/null; then
    AVAILABLE_GB=$(df -BG "$WORK_DIR" 2> /dev/null | tail -1 | awk '{print $4}' | tr -d 'G')
    if [ -n "$AVAILABLE_GB" ] && [ "$AVAILABLE_GB" -lt "$MIN_DISK_GB" ]; then
        echo "ERROR: Insufficient disk space: ${AVAILABLE_GB}GB < ${MIN_DISK_GB}GB required"
        exit 1
    fi
    echo "  Disk space: ${AVAILABLE_GB}GB available (min: ${MIN_DISK_GB}GB)"
fi

# Check 3: Network connectivity to CAS
echo "Checking CAS connectivity..."
if command -v nc &> /dev/null; then
    if ! nc -z -w5 "$CAS_HOST" "$CAS_PORT" 2> /dev/null; then
        echo "ERROR: Cannot connect to CAS at ${CAS_HOST}:${CAS_PORT}"
        exit 1
    fi
    echo "  CAS connectivity: OK (${CAS_HOST}:${CAS_PORT})"
elif command -v timeout &> /dev/null; then
    if ! timeout 5 bash -c "echo > /dev/tcp/${CAS_HOST}/${CAS_PORT}" 2> /dev/null; then
        echo "ERROR: Cannot connect to CAS at ${CAS_HOST}:${CAS_PORT}"
        exit 1
    fi
    echo "  CAS connectivity: OK (${CAS_HOST}:${CAS_PORT})"
else
    echo "  CAS connectivity: SKIPPED (nc/timeout not available)"
fi

# Check 4: GPU availability (optional, for GPU workers)
if [ -n "$CHECK_GPU" ] && [ "$CHECK_GPU" = "true" ]; then
    echo "Checking GPU availability..."
    if command -v nvidia-smi &> /dev/null; then
        if ! nvidia-smi &> /dev/null; then
            echo "ERROR: nvidia-smi failed - GPU not ready"
            exit 1
        fi
        GPU_COUNT=$(nvidia-smi -L 2> /dev/null | wc -l)
        echo "  GPU: OK ($GPU_COUNT GPU(s) available)"
    else
        echo "ERROR: nvidia-smi not found but CHECK_GPU=true"
        exit 1
    fi
fi

# Check 5: Required environment variables (optional)
if [ -n "$REQUIRED_ENV_VARS" ]; then
    echo "Checking required environment variables..."
    for var in $REQUIRED_ENV_VARS; do
        if [ -z "${!var}" ]; then
            echo "ERROR: Required environment variable $var is not set"
            exit 1
        fi
    done
    echo "  Environment variables: OK"
fi

echo ""
echo "All readiness checks passed!"
exit 0
