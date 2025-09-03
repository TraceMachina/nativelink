# Remote Persistent Workers Integration Tests

This directory contains comprehensive integration tests for the NativeLink Remote Persistent Workers feature.

## Overview

Remote Persistent Workers allow NativeLink to keep compiler processes hot by maintaining long-lived worker processes that can handle multiple compilation requests without the overhead of process creation for each action. This is particularly beneficial for JVM-based tools like `javac` and `scalac`.

## Test Structure

```
persistent-workers-test/
├── src/
│   ├── persistent-worker-client.ts    # Client library for interacting with persistent workers
│   ├── mock-scheduler-server.ts       # Mock scheduler for testing
│   ├── persistent-worker.test.ts      # Unit and integration tests
│   └── e2e.test.ts                   # End-to-end test suite
├── package.json                        # Node.js dependencies
├── tsconfig.json                       # TypeScript configuration
└── jest.config.js                      # Jest test runner configuration
```

## Running Tests

### Prerequisites

```bash
# Install dependencies
npm install
```

### Unit Tests

Run the Jest test suite:

```bash
npm test
```

With coverage:

```bash
npm test -- --coverage
```

Watch mode for development:

```bash
npm run test:watch
```

### Integration Tests

Run the full integration test suite:

```bash
npm run test:integration
```

### End-to-End Tests

Build and run E2E tests:

```bash
npm run build
node dist/e2e.test.js
```

## Test Categories

### 1. Persistent Worker Key Extraction
- Tests extraction of `persistentWorkerKey` from platform properties
- Validates handling of missing persistent worker properties

### 2. Worker Lifecycle Management
- **Spawning**: Tests that new persistent workers are spawned when needed
- **Reuse**: Validates that existing workers are reused for matching keys
- **Timeout**: Ensures idle workers are cleaned up after timeout
- **Cleanup**: Verifies proper shutdown of workers

### 3. Scheduler Integration
- **Routing**: Tests that actions are routed to correct persistent workers
- **Platform Matching**: Validates platform property matching logic
- **Worker Pool Management**: Ensures pool size limits are enforced

### 4. Error Handling
- **Spawn Failures**: Tests graceful handling of worker spawn failures
- **Worker Crashes**: Validates automatic spawn after crashes
- **Invalid Tools**: Ensures proper error messages for non-existent tools

### 5. Performance Tests
- **Concurrent Requests**: Tests handling of multiple simultaneous requests
- **Throughput**: Measures actions per second with worker reuse
- **Latency**: Validates low latency for reused workers

## Configuration

The tests can be configured via environment variables:

```bash
# Scheduler endpoint
export SCHEDULER_ENDPOINT=localhost:50051

# Maximum workers in pool
export MAX_WORKERS=10

# Worker idle timeout (seconds)
export WORKER_IDLE_TIMEOUT=300

# Test timeout (milliseconds)
export TEST_TIMEOUT=30000
```

## Debugging

Enable debug logging:

```bash
DEBUG=nativelink:* npm test
```

Run a specific test:

```bash
npm test -- --testNamePattern="should reuse existing persistent worker"
```

## Architecture

The tests validate Nativelink remote persistent workers where:

1. **Runner** manages the lifecycle of persistent workers
2. **Workers** are forked on-demand for specific `persistentWorkerKey` values
3. **Scheduler** routes actions to appropriate persistent workers based on platform properties

### Key Components

1. **PersistentWorkerKey**: Identifies unique persistent worker configurations
2. **PersistentWorkerManager**: Manages pool of persistent worker processes
3. **PersistentWorkerRunner**: Handles forking of new worker processes
4. **Scheduler Integration**: Modified to support persistent worker matching

## Performance Benchmarks

Expected performance characteristics:

- **Cold Start**: ~500-1000ms (initial worker spawn)
- **Warm Request**: <50ms (reused worker)
- **Concurrent Capacity**: 10-20 actions/second per worker
- **Memory Overhead**: ~50-200MB per persistent worker

## Troubleshooting

### Common Issues

1. **Port Already in Use**
   ```
   Error: bind EADDRINUSE 0.0.0.0:50051
   ```
   Solution: Change the port or kill the process using it

2. **Worker Spawn Timeout**
   ```
   Error: Worker spawn timeout exceeded
   ```
   Solution: Increase timeout or check system resources

3. **Protocol Buffer Errors**
   ```
   Error: Cannot find proto file
   ```
   Solution: Ensure proto files are built and paths are correct

## Contributing

When adding new tests:

1. Follow the existing test structure
2. Add both unit and integration tests
3. Update this README with new test descriptions
4. Ensure all tests pass before submitting PR

## License

Apache License 2.0 - See LICENSE file in the root directory.
