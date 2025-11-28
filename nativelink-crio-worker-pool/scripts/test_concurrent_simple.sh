#!/bin/bash
# Simple concurrent jobs test - 10 jobs, 3 workers
set -e

SOCKET="unix:///var/run/crio/crio.sock"
IMAGE="nativelink-worker-java:test"
POOL_SIZE=3
JOBS=10

echo "=== Simple Concurrent Test ==="
echo "Creating $POOL_SIZE workers, running $JOBS jobs"
echo

# Create worker pool
declare -a WORKERS
for i in $(seq 1 $POOL_SIZE); do
    echo -n "Creating worker $i..."

    POD=$(sudo crictl --runtime-endpoint "$SOCKET" runp <(
        cat << EOF
{
  "metadata": {
    "name": "w$i",
    "namespace": "default",
    "uid": "$(uuidgen 2> /dev/null || echo "w$i-$(date +%s)")"
  }
}
EOF
    ) 2> /dev/null)

    CTR=$(sudo crictl --runtime-endpoint "$SOCKET" create "$POD" \
        <(
            cat << EOF
{
  "metadata": {"name": "c$i"},
  "image": {"image": "$IMAGE"},
  "command": ["/bin/sleep", "300"]
}
EOF
        ) \
        <(
            cat << EOF
{
  "metadata": {
    "name": "w$i",
    "namespace": "default",
    "uid": "$(uuidgen 2> /dev/null || echo "w$i-$(date +%s)")"
  }
}
EOF
        ) 2> /dev/null)

    sudo crictl --runtime-endpoint "$SOCKET" start "$CTR" 2> /dev/null
    WORKERS[i]="$CTR"
    echo " ✓"
done

# Warmup workers in parallel
echo
echo "Warming up workers..."
for i in $(seq 1 $POOL_SIZE); do
    (sudo crictl --runtime-endpoint "$SOCKET" exec "${WORKERS[$i]}" \
        /opt/warmup/jvm-warmup.sh > /dev/null 2>&1 || true) &
done
wait
echo "✓ Workers ready"

# Run jobs
echo
echo "Running $JOBS concurrent jobs..."
START=$(date +%s)

for job in $(seq 1 $JOBS); do
    worker_idx=$((((job - 1) % POOL_SIZE) + 1))
    ctr="${WORKERS[$worker_idx]}"

    (
        start=$(date +%s%N)
        sudo crictl --runtime-endpoint "$SOCKET" exec "$ctr" \
            java -version > /dev/null 2>&1
        end=$(date +%s%N)
        time=$(((end - start) / 1000000))
        echo "Job $job (worker $worker_idx): ${time}ms"
    ) &
done

wait
END=$(date +%s)

echo
echo "=== Results ==="
echo "Total time: $((END - START))s"
echo "Throughput: $(echo "scale=1; $JOBS / ($END - $START)" | bc) jobs/sec"
echo "✓ Test complete!"

# Cleanup
echo
echo "Cleaning up..."
for ctr in "${WORKERS[@]}"; do
    sudo crictl --runtime-endpoint "$SOCKET" stop "$ctr" 2> /dev/null || true
    sudo crictl --runtime-endpoint "$SOCKET" rm "$ctr" 2> /dev/null || true
done
sudo crictl --runtime-endpoint "$SOCKET" pods -q |
    xargs -r sudo crictl --runtime-endpoint "$SOCKET" rmp 2> /dev/null || true
echo "✓ Done"
