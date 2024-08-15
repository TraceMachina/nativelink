# Distributed Scheduler Setup:
This example includes six components:
 1. CAS: Shared CAS accessed via GRPC.
 2. Frontend Scheduler: Scheduler with no workers, just places operations into Redis and forwards results to clients once they're completed.
 3. Scheduler-1: A scheduler with a configured worker which reads operations from Redis for assignment.
 4. Scheduler-2: Same as Scheduler:1.
 5. Worker-1: The worker connected to Scheduler-1.
 6. Worker-2: The worker connected to Scheduler-2.

## Cleaning:

- To reset Redis run:
`deployment-examples/redis-scheduler/run.sh clean redis`
- To reset everything run:
`deployment-examples/redis-scheduler/run.sh clean all`

## Starting the Service

### Start CAS:
`./deployment-examples/redis-scheduler/run.sh cas`

### Start Scheduler Frontend:
`./deployment-examples/redis-scheduler/run.sh frontend`

### Start Scheduler-1:
`./deployment-examples/redis-scheduler/run.sh backend_1`

### Start Scheduler-2:
`./deployment-examples/redis-scheduler/run.sh backend_2`
### Start Worker-1:
`./deployment-examples/redis-scheduler/run.sh worker_1`

### Start Worker-2:
`./deployment-examples/redis-scheduler/run.sh worker_2`

## Remote Execution:

```
bazel clean && bazel build //... \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50052 \
  --remote_instance_name="main" \
  --noexperimental_remote_cache_async \
  --platform_suffix=$RANDOM \
  --verbose_failures
```
