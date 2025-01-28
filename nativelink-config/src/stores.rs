// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use serde::{Deserialize, Serialize};

use crate::serde_utils::{
    convert_data_size_with_shellexpand, convert_duration_with_shellexpand,
    convert_numeric_with_shellexpand, convert_optional_string_with_shellexpand,
    convert_string_with_shellexpand, convert_vec_string_with_shellexpand,
};

/// Name of the store. This type will be used when referencing a store
/// in the `CasConfig::stores`'s map key.
pub type StoreRefName = String;

#[allow(non_camel_case_types)]
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum ConfigDigestHashFunction {
    /// Use the sha256 hash function.
    /// <https://en.wikipedia.org/wiki/SHA-2>
    sha256,

    /// Use the blake3 hash function.
    /// <https://en.wikipedia.org/wiki/BLAKE_(hash_function)>
    blake3,
}

#[allow(non_camel_case_types)]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum StoreSpec {
    /// Memory store will store all data in a hashmap in memory.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "memory": {
    ///     "eviction_policy": {
    ///       // 10mb.
    ///       "max_bytes": 10000000,
    ///     }
    ///   }
    /// }
    /// ```
    ///
    memory(MemorySpec),

    /// S3 store will use Amazon's S3 service as a backend to store
    /// the files. This configuration can be used to share files
    /// across multiple instances.
    ///
    /// This configuration will never delete files, so you are
    /// responsible for purging old files in other ways.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "experimental_s3_store": {
    ///   "region": "eu-north-1",
    ///   "bucket": "crossplane-bucket-af79aeca9",
    ///   "key_prefix": "test-prefix-index/",
    ///   "retry": {
    ///     "max_retries": 6,
    ///     "delay": 0.3,
    ///     "jitter": 0.5
    ///   },
    ///   "multipart_max_concurrent_uploads": 10
    /// }
    /// ```
    ///
    experimental_s3_store(S3Spec),

    /// Verify store is used to apply verifications to an underlying
    /// store implementation. It is strongly encouraged to validate
    /// as much data as you can before accepting data from a client,
    /// failing to do so may cause the data in the store to be
    /// populated with invalid data causing all kinds of problems.
    ///
    /// The suggested configuration is to have the CAS validate the
    /// hash and size and the AC validate nothing.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "verify": {
    ///   "memory": {
    ///     "eviction_policy": {
    ///       "max_bytes": 500000000 // 500mb.
    ///     }
    ///   },
    ///   "verify_size": true,
    ///   "hash_verification_function": "sha256"
    /// }
    /// ```
    ///
    verify(Box<VerifySpec>),

    /// Completeness checking store verifies if the
    /// output files & folders exist in the CAS before forwarding
    /// the request to the underlying store.
    /// Note: This store should only be used on AC stores.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "completeness_checking": {
    ///     "backend": {
    ///       "filesystem": {
    ///         "content_path": "~/.cache/nativelink/content_path-ac",
    ///         "temp_path": "~/.cache/nativelink/tmp_path-ac",
    ///         "eviction_policy": {
    ///           // 500mb.
    ///           "max_bytes": 500000000,
    ///         }
    ///       }
    ///     },
    ///     "cas_store": {
    ///       "ref_store": {
    ///         "name": "CAS_MAIN_STORE"
    ///       }
    ///     }
    ///   }
    /// ```
    ///
    completeness_checking(Box<CompletenessCheckingSpec>),

    /// A compression store that will compress the data inbound and
    /// outbound. There will be a non-trivial cost to compress and
    /// decompress the data, but in many cases if the final store is
    /// a store that requires network transport and/or storage space
    /// is a concern it is often faster and more efficient to use this
    /// store before those stores.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "compression": {
    ///     "compression_algorithm": {
    ///       "lz4": {}
    ///     },
    ///     "backend": {
    ///       "filesystem": {
    ///         "content_path": "/tmp/nativelink/data/content_path-cas",
    ///         "temp_path": "/tmp/nativelink/data/tmp_path-cas",
    ///         "eviction_policy": {
    ///           // 2gb.
    ///           "max_bytes": 2000000000,
    ///         }
    ///       }
    ///     }
    ///   }
    /// ```
    ///
    compression(Box<CompressionSpec>),

    /// A dedup store will take the inputs and run a rolling hash
    /// algorithm on them to slice the input into smaller parts then
    /// run a sha256 algorithm on the slice and if the object doesn't
    /// already exist, upload the slice to the `content_store` using
    /// a new digest of just the slice. Once all parts exist, an
    /// Action-Cache-like digest will be built and uploaded to the
    /// `index_store` which will contain a reference to each
    /// chunk/digest of the uploaded file. Downloading a request will
    /// first grab the index from the `index_store`, and forward the
    /// download content of each chunk as if it were one file.
    ///
    /// This store is exceptionally good when the following conditions
    /// are met:
    /// * Content is mostly the same (inserts, updates, deletes are ok)
    /// * Content is not compressed or encrypted
    /// * Uploading or downloading from `content_store` is the bottleneck.
    ///
    /// Note: This store pairs well when used with `CompressionSpec` as
    /// the `content_store`, but never put `DedupSpec` as the backend of
    /// `CompressionSpec` as it will negate all the gains.
    ///
    /// Note: When running `.has()` on this store, it will only check
    /// to see if the entry exists in the `index_store` and not check
    /// if the individual chunks exist in the `content_store`.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "dedup": {
    ///     "index_store": {
    ///       "memory_store": {
    ///         "max_size": 1000000000, // 1GB
    ///         "eviction_policy": "LeastRecentlyUsed"
    ///       }
    ///     },
    ///     "content_store": {
    ///       "compression": {
    ///         "compression_algorithm": {
    ///           "lz4": {}
    ///         },
    ///         "backend": {
    ///           "fast_slow": {
    ///             "fast": {
    ///               "memory_store": {
    ///                 "max_size": 500000000, // 500MB
    ///                 "eviction_policy": "LeastRecentlyUsed"
    ///               }
    ///             },
    ///             "slow": {
    ///               "filesystem": {
    ///                 "content_path": "/tmp/nativelink/data/content_path-content",
    ///                 "temp_path": "/tmp/nativelink/data/tmp_path-content",
    ///                 "eviction_policy": {
    ///                   "max_bytes": 2000000000 // 2gb.
    ///                 }
    ///               }
    ///             }
    ///           }
    ///         }
    ///       }
    ///     }
    ///   }
    /// ```
    ///
    dedup(Box<DedupSpec>),

    /// Existence store will wrap around another store and cache calls
    /// to has so that subsequent `has_with_results` calls will be
    /// faster. This is useful for cases when you have a store that
    /// is slow to respond to has calls.
    /// Note: This store should only be used on CAS stores.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "existence_cache": {
    ///     "backend": {
    ///       "memory": {
    ///         "eviction_policy": {
    ///           // 500mb.
    ///           "max_bytes": 500000000,
    ///         }
    ///       }
    ///     },
    ///     "cas_store": {
    ///       "ref_store": {
    ///         "name": "CAS_MAIN_STORE"
    ///       }
    ///     }
    ///   }
    /// ```
    ///
    existence_cache(Box<ExistenceCacheSpec>),

    /// `FastSlow` store will first try to fetch the data from the `fast`
    /// store and then if it does not exist try the `slow` store.
    /// When the object does exist in the `slow` store, it will copy
    /// the data to the `fast` store while returning the data.
    /// This store should be thought of as a store that "buffers"
    /// the data to the `fast` store.
    /// On uploads it will mirror data to both `fast` and `slow` stores.
    ///
    /// WARNING: If you need data to always exist in the `slow` store
    /// for something like remote execution, be careful because this
    /// store will never check to see if the objects exist in the
    /// `slow` store if it exists in the `fast` store (ie: it assumes
    /// that if an object exists `fast` store it will exist in `slow`
    /// store).
    ///
    /// ***Example JSON Config:***
    /// ```json
    /// "fast_slow": {
    ///     "fast": {
    ///       "filesystem": {
    ///         "content_path": "/tmp/nativelink/data/content_path-index",
    ///         "temp_path": "/tmp/nativelink/data/tmp_path-index",
    ///         "eviction_policy": {
    ///           // 500mb.
    ///           "max_bytes": 500000000,
    ///         }
    ///       }
    ///     },
    ///     "slow": {
    ///       "filesystem": {
    ///         "content_path": "/tmp/nativelink/data/content_path-index",
    ///         "temp_path": "/tmp/nativelink/data/tmp_path-index",
    ///         "eviction_policy": {
    ///           // 500mb.
    ///           "max_bytes": 500000000,
    ///         }
    ///       }
    ///     }
    ///   }
    /// ```
    ///
    fast_slow(Box<FastSlowSpec>),

    /// Shards the data to multiple stores. This is useful for cases
    /// when you want to distribute the load across multiple stores.
    /// The digest hash is used to determine which store to send the
    /// data to.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "shard": {
    ///     "stores": [
    ///         "memory": {
    ///             "eviction_policy": {
    ///                 // 10mb.
    ///                 "max_bytes": 10000000
    ///             },
    ///             "weight": 1
    ///         }
    ///     ]
    /// }
    /// ```
    ///
    shard(ShardSpec),

    /// Stores the data on the filesystem. This store is designed for
    /// local persistent storage. Restarts of this program should restore
    /// the previous state, meaning anything uploaded will be persistent
    /// as long as the filesystem integrity holds. This store uses the
    /// filesystem's `atime` (access time) to hold the last touched time
    /// of the file(s).
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "filesystem": {
    ///     "content_path": "/tmp/nativelink/data-worker-test/content_path-cas",
    ///     "temp_path": "/tmp/nativelink/data-worker-test/tmp_path-cas",
    ///     "eviction_policy": {
    ///       // 10gb.
    ///       "max_bytes": 10000000000,
    ///     }
    /// }
    /// ```
    ///
    filesystem(FilesystemSpec),

    /// Store used to reference a store in the root store manager.
    /// This is useful for cases when you want to share a store in different
    /// nested stores. Example, you may want to share the same memory store
    /// used for the action cache, but use a `FastSlowSpec` and have the fast
    /// store also share the memory store for efficiency.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "ref_store": {
    ///     "name": "FS_CONTENT_STORE"
    /// }
    /// ```
    ///
    ref_store(RefSpec),

    /// Uses the size field of the digest to separate which store to send the
    /// data. This is useful for cases when you'd like to put small objects
    /// in one store and large objects in another store. This should only be
    /// used if the size field is the real size of the content, in other
    /// words, don't use on AC (Action Cache) stores. Any store where you can
    /// safely use `VerifySpec.verify_size = true`, this store should be safe
    /// to use (ie: CAS stores).
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "size_partitioning": {
    ///     "size": 134217728, // 128mib.
    ///     "lower_store": {
    ///       "memory": {
    ///         "eviction_policy": {
    ///           "max_bytes": "${NATIVELINK_CAS_MEMORY_CONTENT_LIMIT:-100000000}"
    ///         }
    ///       }
    ///     },
    ///     "upper_store": {
    ///       /// This store discards data larger than 128mib.
    ///       "noop": {}
    ///     }
    ///   }
    /// ```
    ///
    size_partitioning(Box<SizePartitioningSpec>),

    /// This store will pass-through calls to another GRPC store. This store
    /// is not designed to be used as a sub-store of another store, but it
    /// does satisfy the interface and will likely work.
    ///
    /// One major GOTCHA is that some stores use a special function on this
    /// store to get the size of the underlying object, which is only reliable
    /// when this store is serving the a CAS store, not an AC store. If using
    /// this store directly without being a child of any store there are no
    /// side effects and is the most efficient way to use it.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "grpc": {
    ///     "instance_name": "main",
    ///     "endpoints": [
    ///       {"address": "grpc://${CAS_ENDPOINT:-127.0.0.1}:50051"}
    ///     ],
    ///     "store_type": "ac"
    ///   }
    /// ```
    ///
    grpc(GrpcSpec),

    /// Stores data in any stores compatible with Redis APIs.
    ///
    /// Pairs well with `SizePartitioning` and/or `FastSlow` stores.
    /// Ideal for accepting small object sizes as most redis store
    /// services have a max file upload of between 256Mb-512Mb.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "redis_store": {
    ///     "addresses": [
    ///         "redis://127.0.0.1:6379/",
    ///     ]
    /// }
    /// ```
    ///
    redis_store(RedisSpec),

    /// Noop store is a store that sends streams into the void and all data
    /// retrieval will return 404 (`NotFound`). This can be useful for cases
    /// where you may need to partition your data and part of your data needs
    /// to be discarded.
    ///
    /// **Example JSON Config:**
    /// ```json
    /// "noop": {}
    /// ```
    ///
    noop(NoopSpec),
}

impl StoreSpec {
    // To enforce no duplicate connection configs for a store, add it to the matcher and implement
    // disallow_duplicates_digest() on it. Returns `None` for stores that are not being enforced unique.
    pub fn disallow_duplicates_digest(&self) -> Option<String> {
        match self {
            Self::experimental_s3_store(spec) => Some(spec.disallow_duplicates_digest()),
            Self::filesystem(spec) => Some(spec.disallow_duplicates_digest()),
            Self::grpc(spec) => Some(spec.disallow_duplicates_digest()),
            Self::redis_store(spec) => Some(spec.disallow_duplicates_digest()),
            _ => None,
        }
    }
}

/// Configuration for an individual shard of the store.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ShardConfig {
    /// Store to shard the data to.
    pub store: StoreSpec,

    /// The weight of the store. This is used to determine how much data
    /// should be sent to the store. The actual percentage is the sum of
    /// all the store's weights divided by the individual store's weight.
    ///
    /// Default: 1
    pub weight: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ShardSpec {
    /// Stores to shard the data to.
    pub stores: Vec<ShardConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SizePartitioningSpec {
    /// Size to partition the data on.
    #[serde(deserialize_with = "convert_data_size_with_shellexpand")]
    pub size: u64,

    /// Store to send data when object is < (less than) size.
    pub lower_store: StoreSpec,

    /// Store to send data when object is >= (less than eq) size.
    pub upper_store: StoreSpec,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct RefSpec {
    /// Name of the store under the root "stores" config object.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct FilesystemSpec {
    /// Path on the system where to store the actual content. This is where
    /// the bulk of the data will be placed.
    /// On service bootup this folder will be scanned and all files will be
    /// added to the cache. In the event one of the files doesn't match the
    /// criteria, the file will be deleted.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub content_path: String,

    /// A temporary location of where files that are being uploaded or
    /// deleted will be placed while the content cannot be guaranteed to be
    /// accurate. This location must be on the same block device as
    /// `content_path` so atomic moves can happen (ie: move without copy).
    /// All files in this folder will be deleted on every startup.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub temp_path: String,

    /// Buffer size to use when reading files. Generally this should be left
    /// to the default value except for testing.
    /// Default: 32k.
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub read_buffer_size: u32,

    /// Policy used to evict items out of the store. Failure to set this
    /// value will cause items to never be removed from the store causing
    /// infinite memory usage.
    pub eviction_policy: Option<EvictionPolicy>,

    /// The block size of the filesystem for the running machine
    /// value is used to determine an entry's actual size on disk consumed
    /// For a 4KB block size filesystem, a 1B file actually consumes 4KB
    /// Default: 4096
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub block_size: u64,
}

impl FilesystemSpec {
    fn disallow_duplicates_digest(&self) -> String {
        format!("{}{}", self.content_path, self.temp_path)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct FastSlowSpec {
    /// Fast store that will be attempted to be contacted before reaching
    /// out to the `slow` store.
    pub fast: StoreSpec,

    /// If the object does not exist in the `fast` store it will try to
    /// get it from this store.
    pub slow: StoreSpec,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct MemorySpec {
    /// Policy used to evict items out of the store. Failure to set this
    /// value will cause items to never be removed from the store causing
    /// infinite memory usage.
    pub eviction_policy: Option<EvictionPolicy>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DedupSpec {
    /// Store used to store the index of each dedup slice. This store
    /// should generally be fast and small.
    pub index_store: StoreSpec,

    /// The store where the individual chunks will be uploaded. This
    /// store should generally be the slower & larger store.
    pub content_store: StoreSpec,

    /// Minimum size that a chunk will be when slicing up the content.
    /// Note: This setting can be increased to improve performance
    /// because it will actually not check this number of bytes when
    /// deciding where to partition the data.
    ///
    /// Default: 65536 (64k)
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub min_size: u32,

    /// A best-effort attempt will be made to keep the average size
    /// of the chunks to this number. It is not a guarantee, but a
    /// slight attempt will be made.
    ///
    /// This value will also be about the threshold used to determine
    /// if we should even attempt to dedup the entry or just forward
    /// it directly to the `content_store` without an index. The actual
    /// value will be about `normal_size * 1.3` due to implementation
    /// details.
    ///
    /// Default: 262144 (256k)
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub normal_size: u32,

    /// Maximum size a chunk is allowed to be.
    ///
    /// Default: 524288 (512k)
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub max_size: u32,

    /// Due to implementation detail, we want to prefer to download
    /// the first chunks of the file so we can stream the content
    /// out and free up some of our buffers. This configuration
    /// will be used to to restrict the number of concurrent chunk
    /// downloads at a time per `get()` request.
    ///
    /// This setting will also affect how much memory might be used
    /// per `get()` request. Estimated worst case memory per `get()`
    /// request is: `max_concurrent_fetch_per_get * max_size`.
    ///
    /// Default: 10
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_concurrent_fetch_per_get: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ExistenceCacheSpec {
    /// The underlying store wrap around. All content will first flow
    /// through self before forwarding to backend. In the event there
    /// is an error detected in self, the connection to the backend
    /// will be terminated, and early termination should always cause
    /// updates to fail on the backend.
    pub backend: StoreSpec,

    /// Policy used to evict items out of the store. Failure to set this
    /// value will cause items to never be removed from the store causing
    /// infinite memory usage.
    pub eviction_policy: Option<EvictionPolicy>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct VerifySpec {
    /// The underlying store wrap around. All content will first flow
    /// through self before forwarding to backend. In the event there
    /// is an error detected in self, the connection to the backend
    /// will be terminated, and early termination should always cause
    /// updates to fail on the backend.
    pub backend: StoreSpec,

    /// If set the store will verify the size of the data before accepting
    /// an upload of data.
    ///
    /// This should be set to false for AC, but true for CAS stores.
    #[serde(default)]
    pub verify_size: bool,

    /// If the data should be hashed and verify that the key matches the
    /// computed hash. The hash function is automatically determined based
    /// request and if not set will use the global default.
    ///
    /// This should be set to None for AC, but hashing function like `sha256` for CAS stores.
    #[serde(default)]
    pub verify_hash: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CompletenessCheckingSpec {
    /// The underlying store that will have it's results validated before sending to client.
    pub backend: StoreSpec,

    /// When a request is made, the results are decoded and all output digests/files are verified
    /// to exist in this CAS store before returning success.
    pub cas_store: StoreSpec,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct Lz4Config {
    /// Size of the blocks to compress.
    /// Higher values require more ram, but might yield slightly better
    /// compression ratios.
    ///
    /// Default: 65536 (64k).
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub block_size: u32,

    /// Maximum size allowed to attempt to deserialize data into.
    /// This is needed because the `block_size` is embedded into the data
    /// so if there was a bad actor, they could upload an extremely large
    /// `block_size`'ed entry and we'd allocate a large amount of memory
    /// when retrieving the data. To prevent this from happening, we
    /// allow you to specify the maximum that we'll attempt deserialize.
    ///
    /// Default: value in `block_size`.
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub max_decode_block_size: u32,
}

#[allow(non_camel_case_types)]
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum CompressionAlgorithm {
    /// LZ4 compression algorithm is extremely fast for compression and
    /// decompression, however does not perform very well in compression
    /// ratio. In most cases build artifacts are highly compressible, however
    /// lz4 is quite good at aborting early if the data is not deemed very
    /// compressible.
    ///
    /// see: <https://lz4.github.io/lz4/>
    lz4(Lz4Config),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CompressionSpec {
    /// The underlying store wrap around. All content will first flow
    /// through self before forwarding to backend. In the event there
    /// is an error detected in self, the connection to the backend
    /// will be terminated, and early termination should always cause
    /// updates to fail on the backend.
    pub backend: StoreSpec,

    /// The compression algorithm to use.
    pub compression_algorithm: CompressionAlgorithm,
}

/// Eviction policy always works on LRU (Least Recently Used). Any time an entry
/// is touched it updates the timestamp. Inserts and updates will execute the
/// eviction policy removing any expired entries and/or the oldest entries
/// until the store size becomes smaller than `max_bytes`.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct EvictionPolicy {
    /// Maximum number of bytes before eviction takes place.
    /// Default: 0. Zero means never evict based on size.
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub max_bytes: usize,

    /// When eviction starts based on hitting `max_bytes`, continue until
    /// `max_bytes - evict_bytes` is met to create a low watermark.  This stops
    /// operations from thrashing when the store is close to the limit.
    /// Default: 0
    #[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]
    pub evict_bytes: usize,

    /// Maximum number of seconds for an entry to live since it was last
    /// accessed before it is evicted.
    /// Default: 0. Zero means never evict based on time.
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub max_seconds: u32,

    /// Maximum size of the store before an eviction takes place.
    /// Default: 0. Zero means never evict based on count.
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_count: u64,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct S3Spec {
    /// S3 region. Usually us-east-1, us-west-2, af-south-1, exc...
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub region: String,

    /// Bucket name to use as the backend.
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub bucket: String,

    /// If you wish to prefix the location on s3. If None, no prefix will be used.
    #[serde(default)]
    pub key_prefix: Option<String>,

    /// Retry configuration to use when a network request fails.
    #[serde(default)]
    pub retry: Retry,

    /// If the number of seconds since the `last_modified` time of the object
    /// is greater than this value, the object will not be considered
    /// "existing". This allows for external tools to delete objects that
    /// have not been uploaded in a long time. If a client receives a `NotFound`
    /// the client should re-upload the object.
    ///
    /// There should be sufficient buffer time between how long the expiration
    /// configuration of the external tool is and this value. Keeping items
    /// around for a few days is generally a good idea.
    ///
    /// Default: 0. Zero means never consider an object expired.
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub consider_expired_after_s: u32,

    /// The maximum buffer size to retain in case of a retryable error
    /// during upload. Setting this to zero will disable upload buffering;
    /// this means that in the event of a failure during upload, the entire
    /// upload will be aborted and the client will likely receive an error.
    ///
    /// Default: 5MB.
    pub max_retry_buffer_per_request: Option<usize>,

    /// Maximum number of concurrent `UploadPart` requests per `MultipartUpload`.
    ///
    /// Default: 10.
    pub multipart_max_concurrent_uploads: Option<usize>,

    /// Allow unencrypted HTTP connections. Only use this for local testing.
    ///
    /// Default: false
    #[serde(default)]
    pub insecure_allow_http: bool,

    /// Disable http/2 connections and only use http/1.1. Default client
    /// configuration will have http/1.1 and http/2 enabled for connection
    /// schemes. Http/2 should be disabled if environments have poor support
    /// or performance related to http/2. Safe to keep default unless
    /// underlying network environment or S3 API servers specify otherwise.
    ///
    /// Default: false
    #[serde(default)]
    pub disable_http2: bool,
}

impl S3Spec {
    pub fn disallow_duplicates_digest(&self) -> String {
        let key_prefix = self.key_prefix.as_deref().unwrap_or_default();

        format!("{}{}{}", self.region, self.bucket, key_prefix)
    }
}

#[allow(non_camel_case_types)]
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum StoreType {
    /// The store is content addressable storage.
    cas,
    /// The store is an action cache.
    ac,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ClientTlsConfig {
    /// Path to the certificate authority to use to validate the remote.
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub ca_file: String,

    /// Path to the certificate file for client authentication.
    #[serde(deserialize_with = "convert_optional_string_with_shellexpand")]
    pub cert_file: Option<String>,

    /// Path to the private key file for client authentication.
    #[serde(deserialize_with = "convert_optional_string_with_shellexpand")]
    pub key_file: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct GrpcEndpoint {
    /// The endpoint address (i.e. grpc(s)://example.com:443).
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub address: String,
    /// The TLS configuration to use to connect to the endpoint (if grpcs).
    pub tls_config: Option<ClientTlsConfig>,
    /// The maximum concurrency to allow on this endpoint.
    pub concurrency_limit: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct GrpcSpec {
    /// Instance name for GRPC calls. Proxy calls will have the `instance_name` changed to this.
    #[serde(default, deserialize_with = "convert_string_with_shellexpand")]
    pub instance_name: String,

    /// The endpoint of the grpc connection.
    pub endpoints: Vec<GrpcEndpoint>,

    /// The type of the upstream store, this ensures that the correct server calls are made.
    pub store_type: StoreType,

    /// Retry configuration to use when a network request fails.
    #[serde(default)]
    pub retry: Retry,

    /// Limit the number of simultaneous upstream requests to this many.  A
    /// value of zero is treated as unlimited.  If the limit is reached the
    /// request is queued.
    #[serde(default)]
    pub max_concurrent_requests: usize,

    /// The number of connections to make to each specified endpoint to balance
    /// the load over multiple TCP connections.  Default 1.
    #[serde(default)]
    pub connections_per_endpoint: usize,
}

impl GrpcSpec {
    // todo: could improve duplication detection to individual endpoints to disallow accidental re-use
    fn disallow_duplicates_digest(&self) -> String {
        format!(
            "{}{}",
            self.instance_name,
            self.endpoints
                .clone()
                .into_iter()
                .map(|endpoint| endpoint.address)
                .collect::<Vec<String>>()
                .join(",")
        )
    }
}

/// The possible error codes that might occur on an upstream request.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ErrorCode {
    Cancelled = 1,
    Unknown = 2,
    InvalidArgument = 3,
    DeadlineExceeded = 4,
    NotFound = 5,
    AlreadyExists = 6,
    PermissionDenied = 7,
    ResourceExhausted = 8,
    FailedPrecondition = 9,
    Aborted = 10,
    OutOfRange = 11,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    DataLoss = 15,
    Unauthenticated = 16,
    // Note: This list is duplicated from nativelink-error/lib.rs.
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RedisSpec {
    /// The hostname or IP address of the Redis server.
    /// Ex: `["redis://username:password@redis-server-url:6380/99"]`
    /// 99 Represents database ID, 6380 represents the port.
    #[serde(deserialize_with = "convert_vec_string_with_shellexpand")]
    pub addresses: Vec<String>,

    /// The response timeout for the Redis connection in seconds.
    ///
    /// Default: 10
    #[serde(default)]
    pub response_timeout_s: u64,

    /// The connection timeout for the Redis connection in seconds.
    ///
    /// Default: 10
    #[serde(default)]
    pub connection_timeout_s: u64,

    /// An optional and experimental Redis channel to publish write events to.
    ///
    /// If set, every time a write operation is made to a Redis node
    /// then an event will be published to a Redis channel with the given name.
    /// If unset, the writes will still be made,
    /// but the write events will not be published.
    ///
    /// Default: (Empty String / No Channel)
    #[serde(default)]
    pub experimental_pub_sub_channel: Option<String>,

    /// An optional prefix to prepend to all keys in this store.
    ///
    /// Setting this value can make it convenient to query or
    /// organize your data according to the shared prefix.
    ///
    /// Default: (Empty String / No Prefix)
    #[serde(default)]
    pub key_prefix: String,

    /// Set the mode Redis is operating in.
    ///
    /// Available options are "cluster" for
    /// [cluster mode](https://redis.io/docs/latest/operate/oss_and_stack/reference/cluster-spec/),
    /// "sentinel" for [sentinel mode](https://redis.io/docs/latest/operate/oss_and_stack/management/sentinel/),
    /// or "standard" if Redis is operating in neither cluster nor sentinel mode.
    ///
    /// Default: standard,
    #[serde(default)]
    pub mode: RedisMode,

    /// When using pubsub interface, this is the maximum number of items to keep
    /// queued up before dropping old items.
    ///
    /// Default: 4096
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub broadcast_channel_capacity: usize,

    /// The amount of time in milliseconds until the redis store considers the
    /// command to be timed out. This will trigger a retry of the command and
    /// potentially a reconnection to the redis server.
    ///
    /// Default: 10000 (10 seconds)
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub command_timeout_ms: u64,

    /// The amount of time in milliseconds until the redis store considers the
    /// connection to unresponsive. This will trigger a reconnection to the
    /// redis server.
    ///
    /// Default: 3000 (3 seconds)
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub connection_timeout_ms: u64,

    /// The amount of data to read from the redis server at a time.
    /// This is used to limit the amount of memory used when reading
    /// large objects from the redis server as well as limiting the
    /// amount of time a single read operation can take.
    ///
    /// IMPORTANT: If this value is too high, the `command_timeout_ms`
    /// might be triggered if the latency or throughput to the redis
    /// server is too low.
    ///
    /// Default: 64KiB
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub read_chunk_size: usize,

    /// The number of connections to keep open to the redis server(s).
    ///
    /// Default: 3
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub connection_pool_size: usize,

    /// The maximum number of upload chunks to allow per update.
    /// This is used to limit the amount of memory used when uploading
    /// large objects to the redis server. A good rule of thumb is to
    /// think of the data as:
    /// `AVAIL_MEMORY / (read_chunk_size * max_chunk_uploads_per_update) = THORETICAL_MAX_CONCURRENT_UPLOADS`
    /// (note: it is a good idea to divide `AVAIL_MAX_MEMORY` by ~10 to account for other memory usage)
    ///
    /// Default: 10
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_chunk_uploads_per_update: usize,

    /// Retry configuration to use when a network request fails.
    /// See the `Retry` struct for more information.
    ///
    /// ```txt
    /// Default: Retry {
    ///   max_retries: 0, /* unlimited */
    ///   delay: 0.1, /* 100ms */
    ///   jitter: 0.5, /* 50% */
    ///   retry_on_errors: None, /* not used in redis store */
    /// }
    /// ```
    #[serde(default)]
    pub retry: Retry,
}

impl RedisSpec {
    fn disallow_duplicates_digest(&self) -> String {
        format!("{}{}", self.addresses.clone().join(","), self.key_prefix)
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RedisMode {
    Cluster,
    Sentinel,
    #[default]
    Standard,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct NoopSpec {}

/// Retry configuration. This configuration is exponential and each iteration
/// a jitter as a percentage is applied of the calculated delay. For example:
/// ```haskell
/// Retry{
///   max_retries: 7,
///   delay: 0.1,
///   jitter: 0.5,
/// }
/// ```
/// will result in:
/// Attempt - Delay
/// 1         0ms
/// 2         75ms - 125ms
/// 3         150ms - 250ms
/// 4         300ms - 500ms
/// 5         600ms - 1s
/// 6         1.2s - 2s
/// 7         2.4s - 4s
/// 8         4.8s - 8s
/// Remember that to get total results is additive, meaning the above results
/// would mean a single request would have a total delay of 9.525s - 15.875s.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct Retry {
    /// Maximum number of retries until retrying stops.
    /// Setting this to zero will always attempt 1 time, but not retry.
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_retries: usize,

    /// Delay in seconds for exponential back off.
    #[serde(default)]
    pub delay: f32,

    /// Amount of jitter to add as a percentage in decimal form. This will
    /// change the formula like:
    /// ```haskell
    /// random(
    ///    (2 ^ {attempt_number}) * {delay} * (1 - (jitter / 2)),
    ///    (2 ^ {attempt_number}) * {delay} * (1 + (jitter / 2)),
    /// )
    /// ```
    #[serde(default)]
    pub jitter: f32,

    /// A list of error codes to retry on, if this is not set then the default
    /// error codes to retry on are used.  These default codes are the most
    /// likely to be non-permanent.
    ///  - `Unknown`
    ///  - `Cancelled`
    ///  - `DeadlineExceeded`
    ///  - `ResourceExhausted`
    ///  - `Aborted`
    ///  - `Internal`
    ///  - `Unavailable`
    ///  - `DataLoss`
    #[serde(default)]
    pub retry_on_errors: Option<Vec<ErrorCode>>,
}
