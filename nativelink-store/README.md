# NativeLink Store Module

The `nativelink-store` module is a crucial component of the NativeLink system, providing flexible and extensible storage solutions for the Content Addressable Storage (CAS) system and Action Cache. This module offers various implementations of storage strategies and decorators that can be chained together to create sophisticated storage strategies.

## Overview

 The store module provides the core storage infrastructure that enables NativeLink to efficiently store, retrieve, and manage build, test, and simulation artifacts.

The main idea behind NativeLink stores is the ability to chain them together, building complex behavior from basic constructs. At the end of the chain, there's typically a final store that persists bytes on some durable storage medium like filesystem, S3, or Redis.

## Module Structure

The `nativelink-store` module consists of the following components:

### Core Utility Files

- `ac_utils.rs`: Utilities for the Action Cache functionality.
- `cas_utils.rs`: Utilities for the Content Addressable Storage functionality.
- `store_manager.rs`: Manages the creation and lifecycle of different store implementations.

### Store Implementations

- `memory_store.rs`: An in-memory implementation of the store interface, useful for caching and testing.
- `filesystem_store.rs`: Stores data in the local filesystem using a configurable directory structure.
- `redis_store.rs`: Store implementation that uses Redis as a storage system.
- `gcs_store.rs`: Store implementation for Google Cloud Storage, with the related `gcs_client.rs` providing client functionality.
- `s3_store.rs`: Store implementation for Amazon S3.
- `grpc_store.rs`: Store implementation that forwards requests to another server via gRPC.
- `noop_store.rs`: A no-operation store that does nothing, useful in certain configurations.

### Decorator Stores

- `compression_store.rs`: Applies compression to data before storing it in the underlying store.
- `dedup_store.rs`: Removes duplicate data by storing references to already stored data.
- `existence_cache_store.rs`: Caches existence checks to reduce load on the underlying store.
- `fast_slow_store.rs`: Combines a fast store (like memory) with a slow store (like disk) for tiered access.
- `size_partitioning_store.rs`: Routes data to different stores based on size.
- `shard_store.rs`: Distributes data across multiple stores for load balancing or redundancy.
- `verify_store.rs`: Verifies data integrity when reading from the underlying store.
- `completeness_checking_store.rs`: Ensures that all referenced objects are present in the store.
- `ref_store.rs`: Provides a reference-counting mechanism for store resources.

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
