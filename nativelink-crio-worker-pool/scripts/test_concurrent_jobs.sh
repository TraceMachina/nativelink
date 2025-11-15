#!/bin/bash
# Test concurrent job execution with warm worker pool
set -e

SOCKET="unix:///var/run/crio/crio.sock"
IMAGE="nativelink-worker-java:test"
POOL_SIZE=5
CONCURRENT_JOBS=20

echo "=========================================="
echo "  Concurrent Jobs Test"
echo "=========================================="
echo "Pool size: $POOL_SIZE workers"
echo "Total jobs: $CONCURRENT_JOBS"
echo "Jobs per worker: $((CONCURRENT_JOBS / POOL_SIZE))"
echo

# Create test Java file
cat > /tmp/ConcurrentTest.java << 'EOF'
public class ConcurrentTest {
    public static void main(String[] args) {
        String jobId = args.length > 0 ? args[0] : "unknown";

        // Simulate compilation work
        long start = System.currentTimeMillis();
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < 10000; i++) {
            sb.append("Job ").append(jobId).append(" iteration ").append(i).append("\n");
            if (i % 1000 == 0) {
                sb = new StringBuilder(); // Reset to prevent OOM
            }
        }

        long duration = System.currentTimeMillis() - start;
        System.out.println("Job " + jobId + " completed in " + duration + "ms");
    }
}
EOF

# Step 1: Create and warm up worker pool
echo "Step 1: Creating worker pool ($POOL_SIZE workers)..."
declare -a WORKERS
declare -a PODS

for i in $(seq 1 $POOL_SIZE); do
    echo -n "  Creating worker $i..."

    POD_ID=$(sudo crictl --runtime-endpoint "$SOCKET" runp <(
        cat << EOF
{
  "metadata": {
    "name": "pool-worker-$i",
    "namespace": "default",
    "uid": "worker-$i-$(date +%s)"
  }
}
EOF
    ) 2> /dev/null)

    CONTAINER_ID=$(sudo crictl --runtime-endpoint "$SOCKET" create "$POD_ID" \
        <(
            cat << EOF
{
  "metadata": {"name": "container-$i"},
  "image": {"image": "$IMAGE"},
  "command": ["/bin/sleep", "infinity"]
}
EOF
        ) \
        <(
            cat << EOF
{
  "metadata": {
    "name": "pool-worker-$i",
    "namespace": "default",
    "uid": "worker-$i-$(date +%s)"
  }
}
EOF
        ) 2> /dev/null)

    sudo crictl --runtime-endpoint "$SOCKET" start "$CONTAINER_ID" 2> /dev/null

    # Store worker info
    WORKERS[i]="$CONTAINER_ID"
    PODS[i]="$POD_ID"

    echo " ✓ ($CONTAINER_ID)"
done

echo
echo "Step 2: Warming up workers..."
for i in $(seq 1 $POOL_SIZE); do
    echo -n "  Warming worker $i..."
    container_id="${WORKERS[$i]}"

    # Run warmup in background for speed
    (sudo crictl --runtime-endpoint "$SOCKET" exec "$container_id" \
        /opt/warmup/jvm-warmup.sh > /dev/null 2>&1 || echo "(warmup script not found)") &
done

# Wait for all warmups to complete
wait
echo "  All workers warmed up! ✓"

echo
echo "Step 3: Running $CONCURRENT_JOBS concurrent jobs..."
echo "Started: $(date '+%H:%M:%S')"

START_TIME=$(date +%s)
declare -a JOB_PIDS

# Launch all jobs concurrently
for job in $(seq 1 $CONCURRENT_JOBS); do
    # Round-robin worker assignment
    worker_idx=$((((job - 1) % POOL_SIZE) + 1))
    container_id="${WORKERS[$worker_idx]}"

    # Launch job in background
    (
        job_start=$(date +%s%N)

        # Copy test file to container
        echo "$(< "/tmp/ConcurrentTest.java")" |
            sudo crictl --runtime-endpoint "$SOCKET" exec -i "$container_id" \
                sh -c "cat > /tmp/ConcurrentTest_$job.java" 2> /dev/null

        # Compile and run
        sudo crictl --runtime-endpoint "$SOCKET" exec "$container_id" \
            javac "/tmp/ConcurrentTest_$job.java" 2> /dev/null
        sudo crictl --runtime-endpoint "$SOCKET" exec "$container_id" \
            java -cp /tmp ConcurrentTest "$job" 2> /dev/null

        job_end=$(date +%s%N)
        job_time=$(((job_end - job_start) / 1000000))

        echo "[Worker $worker_idx] Job $job: ${job_time}ms"
        echo "$job_time" > "/tmp/job_${job}_time.txt"
    ) &

    JOB_PIDS[job]=$!

    # Show progress every 5 jobs
    if [ $((job % 5)) -eq 0 ]; then
        echo "  Launched $job/$CONCURRENT_JOBS jobs..."
    fi
done

# Wait for all jobs to complete
echo "  Waiting for all jobs to complete..."
for job in $(seq 1 $CONCURRENT_JOBS); do
    wait "${JOB_PIDS[job]}" 2> /dev/null || true
done

END_TIME=$(date +%s)
TOTAL_TIME=$((END_TIME - START_TIME))

echo "Completed: $(date '+%H:%M:%S')"
echo

# Collect results
echo "Step 4: Analyzing results..."
total_job_time=0
min_time=999999
max_time=0
job_count=0

for job in $(seq 1 $CONCURRENT_JOBS); do
    if [ -f "/tmp/job_${job}_time.txt" ]; then
        time=$(cat "/tmp/job_${job}_time.txt")
        total_job_time=$((total_job_time + time))
        job_count=$((job_count + 1))

        if [ "$time" -lt "$min_time" ]; then
            min_time=$time
        fi
        if [ "$time" -gt "$max_time" ]; then
            max_time=$time
        fi

        rm "/tmp/job_${job}_time.txt"
    fi
done

avg_job_time=$((total_job_time / job_count))

echo
echo "=========================================="
echo "  Results"
echo "=========================================="
echo "Pool configuration:"
echo "  Workers: $POOL_SIZE"
echo "  Total jobs: $CONCURRENT_JOBS"
echo "  Jobs per worker: $((CONCURRENT_JOBS / POOL_SIZE))"
echo
echo "Performance:"
echo "  Total wall time: ${TOTAL_TIME}s"
echo "  Throughput: $(echo "scale=2; $CONCURRENT_JOBS / $TOTAL_TIME" | bc) jobs/second"
echo
echo "Individual job times:"
echo "  Min: ${min_time}ms"
echo "  Max: ${max_time}ms"
echo "  Average: ${avg_job_time}ms"
echo "  Std deviation: ~$((max_time - min_time))ms spread"
echo
echo "Worker efficiency:"
echo "  Total job time: $((total_job_time / 1000))s"
echo "  Wall time: ${TOTAL_TIME}s"
echo "  Parallelism: $(echo "scale=2; $total_job_time / ($TOTAL_TIME * 1000)" | bc)x"
echo "  Efficiency: $(echo "scale=1; ($total_job_time / ($TOTAL_TIME * 1000)) * 100 / $POOL_SIZE" | bc)%"
echo

# Performance analysis
if [ "$avg_job_time" -lt 5000 ]; then
    echo "✅ Excellent! Average job time is under 5 seconds."
elif [ "$avg_job_time" -lt 10000 ]; then
    echo "✓ Good! Average job time is reasonable."
else
    echo "⚠ Warning: Jobs are taking longer than expected."
    echo "   This may indicate workers need more warmup."
fi

if [ $((max_time - min_time)) -lt 2000 ]; then
    echo "✅ Excellent! Job times are very consistent."
elif [ $((max_time - min_time)) -lt 5000 ]; then
    echo "✓ Good! Job times are reasonably consistent."
else
    echo "⚠ Warning: High variance in job times."
    echo "   This may indicate resource contention."
fi

# Cleanup
echo
echo "Step 5: Cleaning up..."
for i in $(seq 1 $POOL_SIZE); do
    container_id="${WORKERS[$i]}"
    pod_id="${PODS[$i]}"

    sudo crictl --runtime-endpoint "$SOCKET" stop "$container_id" 2> /dev/null || true
    sudo crictl --runtime-endpoint "$SOCKET" rm "$container_id" 2> /dev/null || true
    sudo crictl --runtime-endpoint "$SOCKET" stopp "$pod_id" 2> /dev/null || true
    sudo crictl --runtime-endpoint "$SOCKET" rmp "$pod_id" 2> /dev/null || true
done

rm -f /tmp/ConcurrentTest.java

echo "✓ Cleanup complete"
echo
echo "=========================================="
echo "Test completed successfully!"
echo "=========================================="
