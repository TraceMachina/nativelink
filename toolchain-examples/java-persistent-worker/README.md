# Java Remote Persistent Workers Demo

## Overview

This demo showcases **remote persistent workers** in Bazel - a feature that keeps compiler processes (like `javac`) running between builds to eliminate JVM startup overhead. Instead of starting a new JVM for each compilation, the same process handles multiple compilation requests, resulting in **5-21x faster incremental builds**.

## What Are Persistent Workers?

Remote persistent workers are workers that live beyond and independently of a single execution job. It's imperative for reducing Java startup overhead.

### Traditional Compilation (Without Persistent Workers)
```
Build 1: Start JVM → Load javac → Compile → Exit (3-4 seconds)
Build 2: Start JVM → Load javac → Compile → Exit (3-4 seconds)
Build 3: Start JVM → Load javac → Compile → Exit (3-4 seconds)
```

### With Persistent Workers
```
Build 1: Start JVM → Load javac → Compile (3-4 seconds, worker stays alive)
Build 2: Send request to running worker → Compile (< 200ms) ✨
Build 3: Send request to running worker → Compile (< 200ms) ✨
```

## Performance Results

Based on our testing with this demo:

| Build Type | Time | Speed Improvement |
|------------|------|------------------|
| Cold start (no workers) | 3.35s | Baseline |
| First build with workers | 2.87s | 1.2x faster |
| **Incremental build (warm workers)** | **0.16s** | **21x faster!** |

## How to Test

### Prerequisites

1. Bazel 8.4.0 or later (required for proper persistent worker support)
2. Java JDK installed

### Step 1: Configure Bazel for Persistent Workers

The `.bazelrc` file already includes the persistent workers configuration:

```bash
# Persistent workers configuration
build:persistent-workers --strategy=Javac=worker
build:persistent-workers --worker_sandboxing=false
build:persistent-workers --worker_multiplex
build:persistent-workers --worker_multiplex_sandboxing=false
build:persistent-workers --worker_max_instances=Javac=4
build:persistent-workers --worker_quit_after_build=false
```

### Step 2: Run the Performance Test

```bash
# Clean any existing builds and workers
bazel clean
pkill -f "JavaBuilder.*persistent_worker" || true

# Test WITHOUT persistent workers (baseline)
echo "=== Testing WITHOUT persistent workers ==="
time bazel build //java-persistent-worker:calculator_lib

# Clean and test WITH persistent workers (first build)
bazel clean
echo "=== First build WITH persistent workers ==="
time bazel build --config=persistent-workers //java-persistent-worker:calculator_lib

# Make a small change to trigger incremental compilation
echo "// Comment added at $(date)" >> Calculator.java

# Test incremental build with WARM workers (this is where the magic happens!)
echo "=== Incremental build with WARM persistent workers ==="
time bazel build --config=persistent-workers //java-persistent-worker:calculator_lib
```

### Step 3: Verify Workers Are Running

Check that the persistent worker process is alive:

```bash
# Look for the JavaBuilder process with --persistent_worker flag
ps aux | grep -i "JavaBuilder.*persistent_worker" | grep -v grep
```

You should see something like:
```
user 88898 0.0 0.6 422455600 191632 ?? S 3:28PM 0:02.62 .../JavaBuilder_deploy.jar --persistent_worker
```

### Step 4: Watch the Speedup

Run multiple incremental builds and observe the dramatic speedup:

```bash
# Run this a few times - each build should take < 200ms
for i in {1..5}; do
  echo "// Build $i" >> Calculator.java
  time bazel build --config=persistent-workers //java-persistent-worker:calculator_lib 2>&1 | grep "Elapsed"
done
```

## How It Works Under the Hood

1. **First Build**: Bazel starts a JavaBuilder process with `--persistent_worker` flag
2. **Worker Protocol**: Communication happens via protobuf messages (WorkRequest/WorkResponse)
3. **Reuse**: Subsequent builds send compilation requests to the same running process
4. **Multiplexing**: The worker can handle multiple compilation requests concurrently
5. **Lifecycle**: Workers stay alive until manually killed or after idle timeout

## Project Structure

```
java-persistent-worker/
├── BUILD.bazel           # Bazel build configuration with exec_properties
├── Calculator.java       # Math operations library
├── CalculatorTest.java   # JUnit tests
├── StringUtils.java      # String manipulation utilities
├── MainApp.java          # Demo application
└── README.md            # This file
```

## Build Configuration

Each target in `BUILD.bazel` includes persistent worker properties:

```starlark
java_library(
    name = "calculator_lib",
    srcs = ["Calculator.java"],
    exec_properties = {
        "persistentWorkerKey": "javac-calc",    # Unique key for this worker
        "persistentWorkerTool": "javac",         # Tool being used
    },
)
```

## Troubleshooting

### Workers Not Starting?
- Check Bazel version: `bazel version` (need 8.4.0+)
- Kill stale workers: `pkill -f JavaBuilder`
- Check for errors: `bazel build --config=persistent-workers //java-persistent-worker:calculator_lib --verbose_failures`

### Not Seeing Speedup?
- Verify workers are running: `ps aux | grep persistent_worker`
- Check build output for "worker" in process count
- Ensure you're using `--config=persistent-workers` flag

### Remote Execution
For remote execution with NativeLink, persistent workers require:
- NativeLink server with persistent worker support
- Platform properties configured in scheduler
- Worker registration with persistent capabilities

## Benefits

- **5-21x faster** incremental builds
- **Lower CPU usage** - no repeated JVM startup
- **Better developer experience** - near-instant feedback
- **Scalable** - works with large codebases
- **Compatible** - works with standard Bazel toolchains

## Summary

Persistent workers transform the build experience by keeping compiler processes hot. What previously took 3-4 seconds per incremental build now takes less than 200ms. For developers making frequent code changes, this means **waiting minutes instead of hours** over the course of a day.

Try it yourself and experience the dramatic speedup! 🚀
