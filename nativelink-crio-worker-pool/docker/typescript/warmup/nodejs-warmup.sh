#!/bin/bash
set -e

echo "=== Starting Node.js Warmup ==="

# Verify Node.js is available
node --version
npm --version

# Run the warmup script multiple times
echo "Running warmup iterations to trigger V8 TurboFan optimization..."
for i in {1..50}; do
    if [ $((i % 10)) -eq 0 ]; then
        echo "Warmup iteration $i/50"
    fi
    node --expose-gc /opt/warmup/warmup.js > /dev/null 2>&1 || true
done

echo "Final warmup run with output..."
node --expose-gc /opt/warmup/warmup.js

echo "=== Node.js Warmup Complete ==="
