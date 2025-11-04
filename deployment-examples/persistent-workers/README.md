# NativeLink Persistent Workers Deployment Example

This directory contains configuration examples for deploying NativeLink with remote persistent workers support.

## Overview

Remote persistent workers allow build tools to keep compiler processes running between actions, significantly improving build performance for tools like `javac`, `scalac`, and other JVM-based compilers.

## Files

- `nativelink-config.json`: Complete NativeLink scheduler and CAS configuration with persistent worker support
- **.bazelrc**: Bazel configuration for using persistent workers with remote execution
- **nativelink_worker_wrapper.sh**: Wrapper script for starting NativeLink workers with persistent worker support

## Configuration

### Scheduler Configuration

The `nativelink-config.json` includes:
- CAS (Content Addressable Storage) configuration
- Scheduler configuration with worker pools
- Platform property support for `persistentWorkerKey`

### Worker Configuration

Workers must be configured to:
1. Support the `persistentWorkerKey` platform property
2. Use the wrapper script for proper process isolation
3. Have sufficient resources for long-running processes

## Usage

### Starting the Scheduler

```bash
nativelink nativelink-config.json
```

### Starting a Worker

```bash
./nativelink_worker_wrapper.sh --scheduler-endpoint http://localhost:50052
```

### Bazel Client Configuration

Add to your `.bazelrc`:

```
# Enable persistent workers
build --worker_max_instances=4
build --experimental_worker_multiplex

# Remote execution with persistent workers
build --remote_executor=grpc://localhost:50052
build --remote_default_platform_properties=persistentWorkerKey=my-worker-key
build --remote_default_platform_properties=persistentWorkerTool=javac
```

## Platform Properties

Persistent workers are identified by these platform properties:

- `persistentWorkerKey`: Unique identifier for the worker type (for example, `scalac-v2.13`)
- `persistentWorkerTool`: Tool name (for example, `javac`, `scalac`, `kotlinc`)

## Performance Tuning

### Worker Pool Size

Adjust the number of persistent worker instances based on:
- Available memory
- Number of parallel builds
- Compiler startup time

### Idle Timeout

Configure worker idle timeout to balance:
- Resource usage (longer = more memory)
- Warmup time (shorter = more restarts)

Default: 5 minutes

### Maximum Workers

Set maximum concurrent persistent workers:
- Too low: Queue delays
- Too high: Memory exhaustion

Default: 10 workers per key

## Monitoring

Monitor persistent worker health:

```bash
# Check active workers
curl http://localhost:50052/metrics | grep persistent_worker

# View worker statistics
nativelink-admin worker-stats
```

## Troubleshooting

### Workers Not Starting

Check:
1. Platform properties match between client and worker
2. Worker has execute permissions for wrapper script
3. Sufficient memory for persistent processes

### Poor Performance

Investigate:
1. Worker idle timeout too short (frequent restarts)
2. Too few worker instances (queueing)
3. Memory pressure (swapping)

### Memory Leaks

If persistent workers grow over time:
1. Reduce idle timeout
2. Add periodic worker restart
3. Monitor with heap profiling

## Security Considerations

Persistent workers:
- Share process memory across actions
- Must sanitize state between requests
- Should run in isolated namespace

Use the wrapper script to ensure proper isolation.

## See Also

- [Bazel Persistent Workers Documentation](https://bazel.build/remote/persistent)
- [NativeLink Remote Execution Guide](../../../docs/remote-execution.md)
- [Java Example](../../toolchain-examples/java-persistent-worker/)
