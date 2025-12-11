# Test Scripts for CRI-O Warm Workers

Quick testing and benchmarking scripts for evaluating warmed worker performance.

## Quick Start

### 1. Prerequisites Check
```bash
./quick_test.sh
```
This verifies:
- CRI-O is running
- Worker images are available
- Basic container operations work

### 2. Warmup Effectiveness Test
```bash
./test_warmup.sh
```
Measures:
- Cold worker startup time (no warmup)
- Warm worker startup time (with warmup)
- Performance improvement percentage

**Expected Results**:
- Cold worker: ~30-45 seconds
- Warm worker: ~5-10 seconds
- Improvement: ~70-85%

### 3. Concurrent Jobs Test
```bash
# Simple test: 3 workers, 10 jobs
./test_concurrent_simple.sh

# Full test: 5 workers, 20 jobs
./test_concurrent_jobs.sh
```
Measures:
- Pool handling multiple simultaneous jobs
- Worker reuse across jobs
- Performance consistency under load
- Throughput (jobs/second)

**Expected Results**:
- Throughput: ~2-5 jobs/second (3-5 workers)
- Job consistency: ±20% variance
- Efficiency: >80% with proper pool sizing

### 4. Full Benchmark Suite
```bash
./benchmark_all.sh  # Coming soon
```

## Before Running Tests

### Install CRI-O
```bash
sudo apt-get install -y cri-o cri-tools
sudo systemctl start crio
```

### Build Worker Images
```bash
# Java worker
cd ../docker/java
docker build -t nativelink-worker-java:test .

# TypeScript worker
cd ../typescript
docker build -t nativelink-worker-node:test .

# Make images available to CRI-O
sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock \
  pull docker.io/library/nativelink-worker-java:test
```

## Understanding the Results

### Warmup Test Output
```
=== Testing Worker Warmup Effectiveness ===

Test 1: Cold Worker Startup
----------------------------
Cold worker startup time: 45000ms

Test 2: Warm Worker Startup (with warmup)
-----------------------------------------
Running warmup script...
Warm worker startup time: 8000ms

=== Results ===
Cold worker: 45000ms
Warm worker: 8000ms
Improvement: 82%
```

**What this means**:
- Without warmup, the JVM takes 45 seconds to reach full performance
- With warmup, it only takes 8 seconds
- You save 37 seconds (82%) on every worker startup
- With worker reuse, subsequent jobs are instant!

### Metrics to Track

1. **Worker Startup Time**: How long until worker is ready
2. **First Job Time**: How long the first compilation takes
3. **Subsequent Job Time**: Should be consistent and fast
4. **Memory Usage**: Monitor with `crictl stats`
5. **GC Frequency**: How often garbage collection runs

## Troubleshooting

### "CRI-O isn't running"
```bash
sudo systemctl status crio
sudo systemctl start crio
```

### "Worker image not found"
Make sure you built and pushed the image:
```bash
docker build -t nativelink-worker-java:test docker/java/
docker images | grep nativelink
```

### "Permission denied"
The scripts use `sudo` for crictl. Make sure you have sudo access.

### Cleanup stuck containers
```bash
# List all containers
sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock ps -a

# Remove all stopped containers
sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock ps -a -q | \
  xargs -r sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock rm

# Remove all pods
sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock pods -q | \
  xargs -r sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock rmp
```

## See Also

- [TESTING_GUIDE.md](../TESTING_GUIDE.md) - Comprehensive testing documentation
- [PHASE2_SUMMARY.md](../PHASE2_SUMMARY.md) - Implementation overview
- [README.md](../README.md) - Full project documentation
