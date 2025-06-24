---
title: "Building Nativelink with MongoDB: A Nearly Complete Tutorial"
tags: ["news", "blog-posts"]
image: https://nativelink-cdn.s3.us-east-1.amazonaws.com/mongodb_logo.webp
slug: building-nativelink-with-mongodb-storage
pubDate: 2025-06-24
readTime: 15 minutes
---

# Building Nativelink with MongoDB: A Complete Tutorial

We had a lot of customers using Nativelink that were also using MongoDB in various capacities. I also saw these customers over reliant on filesystem stores, rather the durable guarantees and redundancy offered by MongoDB (a place I once worked at for almost 3 years but used for 8 years prior). Given that some of our customers were already using MongoDB and they were using Nativelink, I thought it might be time to experiment putting the two together given customers needs around distributed scheduler state and preferences for databases other than Redis. I want to caution that I haven't benchmarked this implementation against the other stores but can say that there are a lot of features that we will investigate further over time as needs arise.

In this tutorial, the goal is to set up a nearly production-ready Nativelink deployment using MongoDB for both Content Addressable Storage (CAS) and distributed scheduler persistence, then use it to build Nativelink itself.

One of the most powerful demonstrations of a build system is when it can build itself with modular storage databases. The flexibility across storage databases ensures that end users can select the right tool for the job. This showcases both the robustness of the system and provides a flexible configuration example.

## Why MongoDB for Nativelink?

While Nativelink's filesystem store works well for development and smaller deployments, MongoDB offers several advantages for production environments:

- **Durability**: MongoDB provides ACID transactions and configurable write concerns
- **Scalability**: Horizontal scaling through sharding and replica sets
- **Observability**: Rich querying capabilities for monitoring and debugging
- **High Availability**: Built-in replication and automatic failover
- **Network Distribution**: Remote access without complex NFS setups

**Prerequisites:**

Before we begin, ensure you have:

```markdown
1. A MongoDB instance (local or remote) with appropriate credentials
2. Nativelink source code cloned locally
3. Bazel installed for building
4. Basic familiarity with Nativelink configuration
5. One build of Nativelink with Nativelink.
```
## MongoDB Configuration

Below, I've included a complete working configuration that uses MongoDB for the CAS, the action cache, and scheduler storage databases:

[MongoDB config gist](https://gist.github.com/MarcusSorealheis/c5306882c490f9cde3b9806d86f5644f)

In this example, I'm using MongoDB Atlas but you don't need to use it. I like the visualization and monitoring capabilities it offers but there are additional fees associated with using it rather than a self-hosted Community MongoDB cluster. The same configuration should work there.

## Understanding MongoDB Store Behavior

I want to dive a bit deeper into how the behavior of Nativelink's MongoDB store rather than only dump this large configuration JSON on users. First, let's cover how the MongoDB store implementation handles data persistence. Here are two key snippets from the implementation:

### Document Storage Format

```rust
// From [mongo_store.rs](../../../../../nativelink-store/src/mongo_store.rs) - How data is stored in MongoDB
let doc = doc! {
    KEY_FIELD: encoded_key.as_ref(),
    DATA_FIELD: Bson::Binary(mongodb::bson::Binary {
        subtype: mongodb::bson::spec::BinarySubtype::Generic,
        bytes: data,
    }),
    SIZE_FIELD: size,
};

> This approach works for objects up to slightly smaller than 16 MBs. Unless something changed since I last worked at MongoDB, that's the practical limit of how large documents should be before you get into technologies I don't know well and might break behavior, (e.g., GridFS). Ideally 99.999% of documents are nowhere near the 16MB limit, so it's important to be mindful of your use case.

Next, let's dig into write concerns, which ensures durability of what's being written. Of course this level of granularity and control is not useful for everyone but for some users it can be helpful.

// Upsert the document with write concern for durability
let options = UpdateOptions::builder().upsert(true).build();
self.cas_collection
    .update_one(
        doc! { KEY_FIELD: encoded_key.as_ref() },
        doc! { "$set": doc },
        options,
    )
    .await?;
```

This shows how Nativelink stores build artifacts as MongoDB documents. Each piece of data becomes a document with:
- **Key**: The content hash (ensuring deduplication)
- **Data**: The actual binary content stored as BSON Binary
- **Size**: Metadata for quick size checks
- **Upsert**: Ensures idempotent operations (safe to retry)

### Chunked Data Retrieval

Then we can move on to streaming operations that allow us to move data with efficiency.

```rust
// From [mongo_store.rs](../../../../../nativelink-store/src/mongo_store.rs)
let mut cursor = self.cas_collection
    .find(
        doc! { KEY_FIELD: encoded_key.as_ref() },
        FindOptions::builder().limit(1).build(),
    )
    .await?;

// Stream data in chunks for memory efficiency
while let Some(chunk_start) = chunk_stream.try_next().await? {
    let chunk_size = cmp::min(
        self.read_chunk_size,
        (length.unwrap_or(u64::MAX) - bytes_written) as usize,
    );
    // Send chunk to client...
}
```

This demonstrates how the MongoDB store handles large files efficiently by streaming data in configurable chunks rather than loading everything into memory, making it suitable for large build artifacts.

## Creating MongoDB Indexes for Performance

For optimal performance, create these indexes on your MongoDB collections:

```javascript
// Connect to your MongoDB instance and run these commands

// CAS collection indexes
db.cas_data.createIndex({ "_id": 1 }, { unique: true });
db.cas_data.createIndex({ "size": 1 });
db.cas_data.createIndex({ "_id": 1, "size": 1 });

// AC (Action Cache) collection indexes
db.ac_data.createIndex({ "_id": 1 }, { unique: true });
db.ac_data.createIndex({ "size": 1 });

// Scheduler collection indexes
db.scheduler_state.createIndex({ "_id": 1 }, { unique: true });
db.scheduler_state.createIndex({ "version": 1 });
db.scheduler_state.createIndex({ "_id": 1, "version": 1 });

// Optional: TTL index for automatic cleanup (30 days)
// though many users need much longer lifetimes
db.cas_data.createIndex(
  { "createdAt": 1 },
  { expireAfterSeconds: 2592000 }
);
```

As you step through the implementation, I hope you can start to see the enhanced level control offered by a full-featured database like MongoDB.

## Building Nativelink with Nativelink

Now let's use our MongoDB-backed Nativelink setup to build Nativelink itself:

### Step 1: Start the Nativelink Services

Save the configuration from the gist above as `mongo-config.json5` and start Nativelink:

```bash
# Start the Nativelink cluster
./bazel-bin/nativelink mongo-config.json5
```

### Step 2: Configure Bazel for Remote Execution

Create a `.bazelrc` file in your Nativelink source directory:

```bash
# .bazelrc - Configure Bazel to use our MongoDB-backed Nativelink

# Remote cache configuration
build --remote_cache=grpc://localhost:50051
build --remote_upload_local_results=true
build --remote_local_fallback=true

# Remote execution configuration
build --remote_executor=grpc://localhost:50052
build --remote_default_exec_properties=cpu_count=8
build --remote_default_exec_properties=memory_kb=32768

# Ensure consistent builds
build --incompatible_strict_action_env=true
build --action_env=BAZEL_DO_NOT_DETECT_CPP_TOOLCHAIN=1

# Increase timeouts for larger builds
build --remote_timeout=3600
build --remote_cache_compression=true

# Logging for debugging
build --experimental_remote_cache_verbose_failures
build --verbose_failures
```

### Step 3: Build Nativelink

Now build Nativelink using your MongoDB-backed remote execution:

```bash
# Clean build using MongoDB storage
bazel clean --expunge

# Build Nativelink binary with remote execution
bazel build --config=remote //:nativelink

# Run tests with remote caching
bazel test --config=remote //...

# Build all targets
bazel build --config=remote //...
```

### Step 4: Monitor the Build

You can monitor your build's progress by checking MongoDB:

```javascript
// Check CAS storage usage
db.cas_data.aggregate([
  { $group: { _id: null, totalSize: { $sum: "$size" }, count: { $sum: 1 } } }
]);

// Check recent activity
db.cas_data.find().sort({ "_id": -1 }).limit(10);

// Monitor scheduler state
db.scheduler_state.find().sort({ "version": -1 }).limit(5);
```

## Configuration Deep Dive

### Write Concerns

The `write_concern_w: "majority"` setting ensures that writes are acknowledged by a majority of replica set members, providing durability guarantees:

```json5
{
  write_concern_w: "majority",      // Wait for majority acknowledgment
  write_concern_j: true,            // Require journal write
  write_concern_timeout_ms: 5000    // Timeout after 5 seconds
}
```

### Change Streams

Enable change streams for the scheduler to get real-time updates:

```json5
{
  enable_change_streams: true  // Real-time scheduler state updates
}
```

This allows the scheduler to react immediately to worker state changes and job completions.

### Connection Pooling

MongoDB's connection string supports connection pooling:

```
mongodb+srv://user:pass@cluster.mongodb.net/?retryWrites=true&w=majority&maxPoolSize=50&minPoolSize=5
```

## Advantages Over Filesystem Storage

| Feature | MongoDB Store | Filesystem Store |
|-|-|-|
| **Durability** | ACID transactions, write concerns | Depends on filesystem/hardware |
| **Scalability** | Horizontal scaling, sharding | Limited by single machine |
| **Backup** | Native backup tools, point-in-time recovery | Manual file copying |
| **Monitoring** | Rich queries, aggregation pipelines | Basic file system tools |
| **Network Access** | Native remote access | Requires NFS/similar |
| **Deduplication** | Content-addressed by design | Requires additional logic |
| **Consistency** | Configurable consistency levels | Filesystem dependent |


## Production Considerations

Always use authentication and encryption in production:

```
mongodb+srv://user:password@cluster.mongodb.net/?retryWrites=true&w=majority&authSource=admin&ssl=true
```

### Resource Allocation

Configure appropriate MongoDB resources:

```json5
{
  max_concurrent_uploads: 50,     // Balance concurrency vs resources
  read_chunk_size: 65536,         // 64KB chunks for good performance
  connection_timeout_ms: 5000,    // Fast failure detection
  command_timeout_ms: 30000       // Allow time for large operations
}
```

### Monitoring

Set up MongoDB monitoring to track:
- Storage usage and growth rates
- Query performance and slow operations
- Connection pool utilization
- Replication lag (if using replica sets)

## Conclusion

You've now successfully configured Nativelink with MongoDB storage and used it to build itself! This setup demonstrates:

- **Nearly Production-ready configuration** with proper durability settings
- **Scalable storage** that can grow with your build requirements
- **Real-world validation** by building a complex Rust project
- **Performance optimization** through proper indexing

Your MongoDB-backed Nativelink is now nearly ready to handle production workloads. The two details to consider are that (1) this store is still experimental until more of our customers test it out and (2) it's still local. You need to deploy this system in a cluster before you are ready. The recursive build proves the system's reliability and showcases the power of remote execution with persistent, scalable storage.

For next steps, consider setting up MongoDB replica sets if you aren't using the cloud for high availability. Or exploring sharding for even larger scale deployments. While we've implemented a shard store, you won't need to utilize it if using MongoDB's underlying sharding infrastructure in [mongos](https://www.mongodb.com/docs/manual/reference/program/mongos/).

I'm open to feedback on this approach. Let me know if you'd like to see other databases. Otherwise, stay tuned for the next one. Thanks for reading!
