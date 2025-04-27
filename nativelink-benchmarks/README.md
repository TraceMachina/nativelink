# NativeLink Benchmarks

This repository contains the code for the NativeLink per-commit benchmarking system, inspired by the [Lucene Nightly Benchmarks project](https://benchmarks.mikemccandless.com/).

## Overview

The NativeLink benchmarking system measures and tracks performance across different commits, allowing developers to identify performance regressions or improvements. The system focuses on several key use cases:

1. **Build with remote cache only** - Tests how NativeLink performs when used solely as a remote cache
2. **Build with remote cache and execution** - Tests combined performance of remote caching and execution

## Features

- Per-commit performance tracking
- Visualization of performance trends over time
- Regression detection between commits
- Integration with CI/CD pipelines
- Support for various test projects (C++ and Rust with Bazel)

## Benchmark Metrics

The system collects the following metrics:

- Build time
- Resource usage (CPU, memory)
- Network I/O
- Cache hit/miss rates

## Accessing Benchmarks

The benchmark results are available at: [https://benchmarks.nativelink.dev](https://benchmarks.nativelink.dev)

## Contributing

Contributions to improve the benchmarking system are welcome. Please see the [CONTRIBUTING.md](CONTRIBUTING.md) file for guidelines.

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.