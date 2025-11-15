# Concurrent Jobs Testing Guide

## Quick Overview

We've provided two concurrent job tests to verify the warm worker pool handles multiple simultaneous builds:

| Test | Workers | Jobs | Runtime | Purpose |
|------|---------|------|---------|---------|
| **Quick** | 3 | 10 | ~30s | Quick validation |
| **Full** | 5 | 20 | ~2m | Comprehensive benchmark |

## Test 1: Quick Concurrent Test

**Run it:**
```bash
./test_concurrent_simple.sh
```

**What it does:**
```
┌─────────────────────────────────────┐
│  Worker Pool (3 workers)            │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐│
│  │Worker 1 │ │Worker 2 │ │Worker 3 ││
│  └─────────┘ └─────────┘ └─────────┘│
└─────────────────────────────────────┘
         ↑          ↑          ↑
         │          │          │
    ┌────┴────┬────┴────┬────┴────┐
    │ Job 1   │ Job 2   │ Job 3   │
    │ Job 4   │ Job 5   │ Job 6   │
    │ Job 7   │ Job 8   │ Job 9   │
    │ Job 10  │         │         │
    └─────────┴─────────┴─────────┘
```

**Expected Output:**
```
=== Simple Concurrent Test ===
Creating 3 workers, running 10 jobs

Creating worker 1... ✓
Creating worker 2... ✓
Creating worker 3... ✓

Warming up workers...
✓ Workers ready

Running 10 concurrent jobs...
Job 1 (worker 1): 145ms
Job 2 (worker 2): 152ms
Job 3 (worker 3): 148ms
Job 4 (worker 1): 143ms
Job 5 (worker 2): 147ms
Job 6 (worker 3): 151ms
Job 7 (worker 1): 144ms
Job 8 (worker 2): 149ms
Job 9 (worker 3): 146ms
Job 10 (worker 1): 145ms

=== Results ===
Total time: 3s
Throughput: 3.3 jobs/sec
✓ Test complete!
```

**What to look for:**
- ✅ All jobs complete successfully
- ✅ Job times are consistent (~±10ms variance)
- ✅ Throughput: 2-4 jobs/second
- ✅ Workers are reused (job 4, 7, 10 all use worker 1)

## Test 2: Full Concurrent Test

**Run it:**
```bash
./test_concurrent_jobs.sh
```

**What it does:**
```
┌────────────────────────────────────────────────┐
│  Worker Pool (5 workers)                       │
│  ┌────┐ ┌────┐ ┌────┐ ┌────┐ ┌────┐          │
│  │ W1 │ │ W2 │ │ W3 │ │ W4 │ │ W5 │          │
│  └────┘ └────┘ └────┘ └────┘ └────┘          │
└────────────────────────────────────────────────┘
    ↑      ↑      ↑      ↑      ↑
    │      │      │      │      │
20 jobs distributed round-robin:
- Worker 1: jobs 1, 6, 11, 16
- Worker 2: jobs 2, 7, 12, 17
- Worker 3: jobs 3, 8, 13, 18
- Worker 4: jobs 4, 9, 14, 19
- Worker 5: jobs 5, 10, 15, 20
```

**Expected Output:**
```
==========================================
  Concurrent Jobs Test
==========================================
Pool size: 5 workers
Total jobs: 20
Jobs per worker: 4

Step 1: Creating worker pool (5 workers)...
  Creating worker 1... ✓ (abc123...)
  Creating worker 2... ✓ (def456...)
  Creating worker 3... ✓ (ghi789...)
  Creating worker 4... ✓ (jkl012...)
  Creating worker 5... ✓ (mno345...)

Step 2: Warming up workers...
  All workers warmed up! ✓

Step 3: Running 20 concurrent jobs...
Started: 10:15:30
  Launched 5/20 jobs...
  Launched 10/20 jobs...
  Launched 15/20 jobs...
  Launched 20/20 jobs...
  Waiting for all jobs to complete...
[Worker 1] Job 1: 2450ms
[Worker 2] Job 2: 2380ms
[Worker 3] Job 3: 2420ms
[Worker 4] Job 4: 2410ms
[Worker 5] Job 5: 2390ms
[Worker 1] Job 6: 2100ms  <-- Faster! Worker already warm
[Worker 2] Job 7: 2090ms
[Worker 3] Job 8: 2110ms
[Worker 4] Job 9: 2105ms
[Worker 5] Job 10: 2095ms
[Worker 1] Job 11: 2085ms
[Worker 2] Job 12: 2080ms
[Worker 3] Job 13: 2090ms
[Worker 4] Job 14: 2088ms
[Worker 5] Job 15: 2082ms
[Worker 1] Job 16: 2078ms
[Worker 2] Job 17: 2081ms
[Worker 3] Job 18: 2079ms
[Worker 4] Job 19: 2083ms
[Worker 5] Job 20: 2077ms
Completed: 10:15:48

Step 4: Analyzing results...

==========================================
  Results
==========================================
Pool configuration:
  Workers: 5
  Total jobs: 20
  Jobs per worker: 4

Performance:
  Total wall time: 18s
  Throughput: 1.11 jobs/second

Individual job times:
  Min: 2077ms
  Max: 2450ms
  Average: 2154ms
  Std deviation: ~373ms spread

Worker efficiency:
  Total job time: 43s
  Wall time: 18s
  Parallelism: 2.39x
  Efficiency: 47.8%

✅ Excellent! Average job time is under 5 seconds.
✅ Excellent! Job times are very consistent.

Step 5: Cleaning up...
✓ Cleanup complete

==========================================
Test completed successfully!
==========================================
```

**What to look for:**
- ✅ First job per worker: Slightly slower (~2400ms)
- ✅ Subsequent jobs: Faster and consistent (~2100ms)
- ✅ Job variance: < 500ms spread
- ✅ Parallelism: > 2x (shows workers running concurrently)
- ✅ Efficiency: > 40% (with 5 workers, theoretical max is 100%)

## Understanding the Metrics

### 1. Throughput (jobs/second)
```
Throughput = Total Jobs / Wall Time
```
**Expected**: 1-3 jobs/sec (depends on job complexity)

With 5 workers and 2-second jobs, theoretical max is 2.5 jobs/sec.
You'll get 60-80% of theoretical max in practice.

### 2. Parallelism
```
Parallelism = Total Job Time / Wall Time
```
**Expected**: Close to number of workers

- 5 workers → ~4-5x parallelism (perfect)
- Lower → Workers sitting idle
- Higher → Impossible (something's wrong with measurement)

### 3. Efficiency
```
Efficiency = (Parallelism / Worker Count) × 100%
```
**Expected**: > 80% with good pool sizing

- 80-100%: Excellent! Workers fully utilized
- 50-80%: Good, some idle time
- < 50%: Workers underutilized or jobs too fast

### 4. Job Time Consistency
```
Variance = Max Time - Min Time
```
**Expected**: < 30% of average

- < 20%: Excellent consistency
- 20-40%: Acceptable
- > 40%: High variance, investigate GC or resource contention

## Common Issues & Solutions

### Issue: Jobs take too long
```
Average job time: 10000ms  (too slow!)
```
**Causes**:
- Workers not properly warmed up
- Resource contention (CPU/memory)
- Swap being used

**Solutions**:
```bash
# Check warmup actually ran
sudo crictl exec $CONTAINER_ID java -version  # Should be fast

# Check resource usage
sudo crictl stats

# Increase warmup iterations
# Edit docker/java/warmup/jvm-warmup.sh: 100 → 200 iterations
```

### Issue: High variance in job times
```
Min: 2000ms, Max: 8000ms  (±300% variance!)
```
**Causes**:
- GC pauses
- Memory pressure
- Some workers not warmed

**Solutions**:
```bash
# Check GC activity
sudo crictl exec $CONTAINER_ID jcmd 1 GC.heap_info

# Force GC between jobs
sudo crictl exec $CONTAINER_ID jcmd 1 GC.run

# Increase memory limits in container config
```

### Issue: Low parallelism/efficiency
```
Parallelism: 1.2x with 5 workers  (should be ~4-5x!)
Efficiency: 24%
```
**Causes**:
- Jobs completing too quickly
- Workers not all running simultaneously
- Measurement timing issues

**Solutions**:
```bash
# Use longer-running jobs
# Edit ConcurrentTest.java: 10000 → 100000 iterations

# Verify all workers started
sudo crictl ps | grep running

# Check if jobs are actually parallel
# Add logging to see timestamps
```

## Visual: What Happens During the Test

```
Time →

0s    [Create 5 workers]                    ┐
      W1 W2 W3 W4 W5                        │ Setup
10s   [Warmup all workers in parallel]     │ Phase
      🔥🔥🔥🔥🔥                            ┘

18s   [Launch all 20 jobs]                  ┐
      W1: J1  J6  J11 J16                   │
      W2: J2  J7  J12 J17                   │
      W3: J3  J8  J13 J18                   │ Execution
      W4: J4  J9  J14 J19                   │ Phase
      W5: J5  J10 J15 J20                   │
      ↓   ↓   ↓   ↓                         │
36s   [All jobs complete]                   ┘

38s   [Cleanup]                             → Cleanup
```

## Next Steps

After running these tests:

1. **Compare with cold workers**: Run without warmup to see the difference
2. **Tune pool size**: Try different worker counts (3, 5, 10)
3. **Stress test**: Increase to 50+ jobs
4. **Real workloads**: Test with actual compilation tasks
5. **Monitor metrics**: Track over time in production

## Quick Reference

```bash
# Quick test (30 seconds)
./test_concurrent_simple.sh

# Full test (2-3 minutes)
./test_concurrent_jobs.sh

# With custom settings
POOL_SIZE=10 CONCURRENT_JOBS=50 ./test_concurrent_jobs.sh

# Clean up if test fails
sudo crictl ps -a -q | xargs -r sudo crictl rm
sudo crictl pods -q | xargs -r sudo crictl rmp
```

Ready to test? Start with the quick version:
```bash
cd nativelink-crio-worker-pool/scripts
./test_concurrent_simple.sh
```
