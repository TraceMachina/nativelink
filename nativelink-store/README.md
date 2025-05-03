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

### Utilities

- `redis_utils.rs`: Utilities for Redis store operations (private module).

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

## Key Features

- **Flexible Storage Options**: Multiple storage backends to suit different deployment environments.
<!-- vale Vale.Spelling = NO -->
- **Composable Architecture**: Stores can be combined and decorated to create sophisticated caching and storage strategies.
<!-- vale Vale.Spelling = YES -->
- **Performance Optimizations**: Multiple strategies for improving performance, including caching, compression, and size-based routing.
- **Distributed Storage Support**: Support for distributed stores like Redis, S3, and GCS.
- **Integrity Verification**: Built-in mechanisms for ensuring data integrity.

## Integration

The store module integrates with the rest of NativeLink through the store interfaces defined in the codebase. It's used by both the CAS service and Action Cache service to store and retrieve build artifacts, action results, and other data.

## Configuration

Store configurations are typically defined in JSON configuration files.

Example configuration patterns can be seen in the [NativeLink Config README](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/README.md).

## Adding a New Store Implementation

NativeLink's store architecture is designed to be extensible. Adding a new store implementation involves several steps, which are outlined below. This process is based on real examples from the project's history.

### Step 1: Decide on Implementation Approach

First, determine if your store will be a completely new implementation or if it can leverage an existing pattern. For example, the Google Cloud Storage implementation built upon the generic patterns established for the S3 store.

### Step 2: Create the Generic Pattern (if needed)

If your implementation is similar to existing stores but needs a more generic pattern:

1. Extract common patterns into generic interfaces and helper structures
2. Ensure the original implementations continue to work
3. Document the generic patterns for reuse

A good example of this was PR [#1720](https://github.com/TraceMachina/nativelink/pull/1720), which changed the S3Store into a generic CloudObjectStore pattern that could be used for implementing other cloud storage providers.

### Step 3: Implement the Store

Implement the core functionality of your store by:

1. Creating a new file for your store implementation (for example, `gcs_store.rs`)
2. Defining the store struct and implementing the necessary traits:
   - `StoreTrait` for basic CAS operations
   - `AsAny` for type conversions
   - Other utility traits like `Debug`, `Display`, etc.
3. Implementing helper modules if needed for complex functionality
4. Writing thorough unit tests to validate your implementation

An example of this was PR [#1645](https://github.com/TraceMachina/nativelink/pull/1645), which implemented the Google Cloud Storage store using the generic CloudObjectStore pattern.

### Step 4: Add Configuration Support

Add configuration structs in the `nativelink-config` crate to:
1. Define the configuration schema for your store
<!-- vale Vale.Spelling = NO -->
2. Implement deserialization from JSON/YAML configuration
<!-- vale Vale.Spelling = YES -->
3. Include appropriate defaults where sensible

### Step 5: Register the Store Factory

Update the `default_store_factory.rs` to include a factory method for creating instances of your store:

```rust
// Example for registering a new store factory
pub fn create_mystore(
    spec: &MyStoreSpec,
    config_path: Option<&Path>,
) -> Result<Arc<dyn StoreTrait>> {
    // Implementation details...
    Ok(Arc::new(MyStore::new(/* parameters */)?))
}
```

### Step 6: Testing Considerations

- Implement thorough unit tests for your store
- Add integration tests if applicable
- Consider mocking external dependencies for testing
- Document any specific testing requirements

### Best Practices

- Follow the existing code style and patterns
- Minimize dependencies, especially on external services
- Use configuration options with reasonable defaults
- Consider performance implications, especially for operations that might be called frequently
- Handle errors gracefully and provide meaningful error messages
- Document your implementation thoroughly, including example configurations

### Real-World Examples

For real-world examples of adding new stores to NativeLink, see:

1. **Generic Store Pattern Creation**: [PR #1720](https://github.com/TraceMachina/nativelink/pull/1720) - Changed S3Store to a generic CloudObjectStore
2. **New Store Implementation**: [PR #1645](https://github.com/TraceMachina/nativelink/pull/1645) - Implemented Google Cloud Storage with REST

These PRs demonstrate the process of extending the store system with new implementations, including the review process and considerations for performance, maintainability, and code quality.

---

For more detailed information on specific store implementations or configuration options, please refer to the implementation files and the [NativeLink documentation](https://www.nativelink.com/docs/config/configuration-intro).
