#!/bin/bash

# === Config ===
TARGET="//:nativelink"
LOG_FILE="benchmark_results.csv"

# === Ensure log file has headers ===
if [ ! -f "$LOG_FILE" ]; then
    echo "Date,Commit,Duration(s)" > "$LOG_FILE"
fi

# === Capture commit hash ===
COMMIT_HASH=$(git rev-parse --short HEAD)

# === Run benchmark ===
echo "Running benchmark for $TARGET..."
START_TIME=$(date +%s.%N)

bazel clean > /dev/null
bazel build "$TARGET"

END_TIME=$(date +%s.%N)

# === Calculate duration ===
DURATION=$(echo "$END_TIME - $START_TIME" | bc)
DATE=$(date '+%Y-%m-%d %H:%M:%S')

# === Log result ===
echo "$DATE,$COMMIT_HASH,$DURATION" >> "$LOG_FILE"
echo "✅ Benchmark completed: $DURATION seconds"
