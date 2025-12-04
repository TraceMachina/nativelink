#!/bin/bash
set -e

echo "=== Starting JVM Warmup ==="

# Run the warmup class multiple times to trigger JIT compilation
echo "Running warmup iterations to trigger JIT compilation..."
java -cp /opt/warmup WarmupRunner 100

# Load common Java build tool classes (if available)
echo "Loading common build tool classes..."
java -version 2>&1 | head -n 1

# Exercise jcmd (used for GC triggering)
echo "Testing jcmd availability..."
jcmd -l || echo "jcmd not available for current process"

echo "=== JVM Warmup Complete ==="
