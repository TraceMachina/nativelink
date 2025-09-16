#!/bin/bash
# Copyright 2025 The NativeLink Authors. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# NativeLink Worker Wrapper Script for Persistent Workers
# This script wraps the nativelink binary to handle persistent worker operations

set -euo pipefail

# Configuration
NATIVELINK_BIN="${NATIVELINK_BIN:-/nativelink/bin/nativelink}"
WORKER_CONFIG="${WORKER_CONFIG:-/nativelink/config/nativelink-config.json}"
LOG_LEVEL="${LOG_LEVEL:-info}"
WORKER_ID="${WORKER_ID:-$(hostname)-$$}"

# Logging function
log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] [WRAPPER] $*" >&2
}

# Error handling
handle_error() {
    log "ERROR: Command failed with exit code $?"
    exit 1
}

trap handle_error ERR

# Validate environment
if [ ! -x "$NATIVELINK_BIN" ]; then
    log "ERROR: NativeLink binary not found or not executable at: $NATIVELINK_BIN"
    exit 1
fi

if [ ! -r "$WORKER_CONFIG" ]; then
    log "ERROR: Worker config file not found or not readable at: $WORKER_CONFIG"
    exit 1
fi

# Create necessary directories
WORK_DIR="${WORK_DIR:-/tmp/nativelink/work}"
mkdir -p "$WORK_DIR"
mkdir -p /tmp/nativelink/content_path-cas
mkdir -p /tmp/nativelink/tmp_path-cas
mkdir -p /tmp/nativelink/content_path-ac
mkdir -p /tmp/nativelink/tmp_path-ac

log "Starting NativeLink worker with persistent worker support"
log "Worker ID: $WORKER_ID"
log "Config: $WORKER_CONFIG"
log "Work directory: $WORK_DIR"
log "Log level: $LOG_LEVEL"

# Set environment variables for persistent worker support
export RUST_LOG="${RUST_LOG:-$LOG_LEVEL}"
export NATIVELINK_WORKER_ID="$WORKER_ID"
export NATIVELINK_PERSISTENT_WORKER_ENABLED="true"

# Handle graceful shutdown
cleanup() {
    log "Received shutdown signal, cleaning up..."
    # Kill any child processes
    pkill -P $$ || true
    log "Shutdown complete"
    exit 0
}

trap cleanup SIGTERM SIGINT

# Start the NativeLink worker
log "Executing: $NATIVELINK_BIN $WORKER_CONFIG"
exec "$NATIVELINK_BIN" "$WORKER_CONFIG"
