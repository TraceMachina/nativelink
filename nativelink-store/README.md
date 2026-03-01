# NativeLink Store Module

The `nativelink-store` module is a crucial component of the NativeLink system, providing flexible and extensible storage solutions for the Content Addressable Storage (CAS) system and Action Cache. This module offers various implementations of storage strategies and decorators that can be chained together to create sophisticated storage strategies.

## Overview

 The store module provides the core storage infrastructure that enables NativeLink to efficiently store, retrieve, and manage build, test, and simulation artifacts.

The main idea behind NativeLink stores is the ability to chain them together, building complex behavior from basic constructs. At the end of the chain, there's typically a final store that persists bytes on some durable storage medium like filesystem, S3, or Redis.


## Usage Patterns
<!-- vale Vale.Spelling = NO -->
The store module is designed with a composable pattern in mind. Store implementations can be wrapped by decorator stores to add functionality. For example:
<!-- vale Vale.Spelling = YES -->

1. **Basic In-Memory Store**: For development or testing, using just the `memory_store.rs`.

2. **Production CAS Configuration**: A more complex setup might include:

   - `fast_slow_store.rs` as the top level, combining:
     - A fast `memory_store.rs` for frequently accessed data
     - A `size_partitioning_store.rs` that routes data based on size to:
       - `redis_store.rs` for smaller objects
       - `compression_store.rs` + `s3_store.rs` for larger objects

3. **Enhanced Reliability**: Adding verification and existence checking:
   - `verify_store.rs` wrapping
   - `existence_cache_store.rs` wrapping
   - `filesystem_store.rs`


## Configuration

Store configurations are typically defined in JSON configuration files.

Example configuration patterns can be seen in the [NativeLink Config README](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/README.md).

## Adding a New Store Implementation

NativeLink's store architecture is designed to be extensible. Adding a new store implementation involves several steps, which are outlined below. This process is based on real examples from the project's history.

### Real-World Examples

For real-world examples of adding new stores to NativeLink, see:

1. **Generic Store Pattern Creation**: [PR #1720](https://github.com/TraceMachina/nativelink/pull/1720) - Changed S3Store to a generic CloudObjectStore
2. **New Store Implementation**: [PR #1645](https://github.com/TraceMachina/nativelink/pull/1645) - Implemented Google Cloud Storage with REST

These PRs demonstrate the process of extending the store system with new implementations, including the review process and considerations for performance, maintainability, and code quality.

---

For more detailed information on specific store implementations or configuration options, please refer to the implementation files and the [NativeLink documentation](https://www.nativelink.com/docs/config/configuration-intro).
