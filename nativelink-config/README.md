# Configuration examples

NativeLink uses a JSON file as the configuration format.

To view the available fields please refer to the [configuration reference](https://github.com/tracemachina/nativelink/tree/main/nativelink-config/src).

## Examples

The [examples directory](https://github.com/tracemachina/nativelink/tree/master/nativelink-config/examples) contains a few examples of configuration files.

A very basic configuration that's a pure in-memory store is:

```js
{
  "stores": [
    {
      "name": "CAS_MAIN_STORE",
      "memory": {
        "eviction_policy": {
          // 1gb.
          "max_bytes": 1000000000,
        }
      }
    },
    {
      "name": "AC_MAIN_STORE",
      "memory": {
        "eviction_policy": {
          // 100mb.
          "max_bytes": 100000000,
        }
      }
    }
  ],
  "servers": [{
    "listener": {
      "http": {
        "socket_address": "0.0.0.0:50051",
        "advanced_http": {
          "http2_keep_alive_interval": 10
        }
      }
    },
    "services": {
      "cas": {
        "main": {
          "cas_store": "CAS_MAIN_STORE"
        }
      },
      "ac": {
        "main": {
          "ac_store": "AC_MAIN_STORE"
        }
      },
      "capabilities": {
        "main": {}
      },
      "bytestream": {
        "cas_stores": {
          "main": "CAS_MAIN_STORE",
        }
      }
    }
  }]
}
```

### Compression Store

The following configuration will cause the underlying data to be backed by the
filesystem, and when the number of bytes reaches over 100mb for AC objects and
10gb for CAS objects evict them, but apply lz4 compression on the data before
sending it to be stored. This will also automatically decompress the data when
the data is retrieved.

```js
{
  "stores": [
    {
      "name": "CAS_MAIN_STORE",
      "compression": {
        "compression_algorithm": {
          "lz4": {}
        },
        "backend": {
          "filesystem": {
            "content_path": "/tmp/bazel_cache/cas",
            "temp_path": "/tmp/bazel_cache/tmp_data",
            "eviction_policy": {
              // 10gb.
              "max_bytes": 10000000000,
            }
          }
        }
      }
    },
    {
      "name": "AC_MAIN_STORE",
      "filesystem": {
        "content_path": "/tmp/bazel_cache/ac",
        "temp_path": "/tmp/bazel_cache/tmp_data",
        "eviction_policy": {
          // 100mb.
          "max_bytes": 100000000,
        }
      }
    }
  ],
  // Place rest of configuration here ...
}
```

<!-- vale off -->
### Dedup Store
<!-- vale on -->

In this example we will attempt to de-duplicate our data and compress it before
storing it. This works by applying the [FastCDC](https://www.usenix.org/system/files/conference/atc16/atc16-paper-xia.pdf)
window-based rolling checksum algorithm on the data, splitting the data into
smaller pieces then storing each chunk as an individual entry in another store.

This is very useful when large objects are stored and only parts of the
object/file are modified. Examples, are multiple large builds that have debug
information in them. It's very common for large binary objects that contain
debug information to be almost identical when only a subset of modules are
changed. In enterprise level systems this will likely add up to huge
efficiencies, since if just a few bytes are added/removed or changed it will
only transfer the bytes around where the changes occurred.

```js
{
  "stores": [
    {
      "name": "CAS_MAIN_STORE",
      "dedup": {
        // Index store contains the references to the chunks of data and how to
        // reassemble them live. These will usually be <1% of the total size of
        // the object being indexed.
        "index_store": {
          "filesystem": {
            "content_path": "/tmp/bazel_cache/cas-index",
            "temp_path": "/tmp/bazel_cache/tmp_data",
            "eviction_policy": {
              // 100mb.
              "max_bytes": 100000000,
            }
          }
        },
        // This is where the actual content will be stored, but will be in small
        // files chunked into different sizes based on the "*_size" settings below.
        "content_store": {
          // Then apply a compression configuration to the individual file chunks.
          "compression": {
            "compression_algorithm": {
              "lz4": {}
            },
            "backend": {
              // Then take those compressed chunks and store them to the filesystem.
              "filesystem": {
                "content_path": "/tmp/bazel_cache/cas",
                "temp_path": "/tmp/bazel_cache/tmp_data",
                "eviction_policy": {
                  // 10gb.
                  "max_bytes": 10000000000,
                }
              }
            }
          }
        },
        // The file will not be chunked into parts smaller than this (64k).
        "min_size": 65536,
        // The file will attempt to be chunked into about this size (128k).
        "normal_size": 131072,
        // No chunk should be larger than this size (256k).
        "max_size": 262144
      }
    },
    {
      "name": "AC_MAIN_STORE",
      // Don't apply anything special to our action cache, just store as normal files.
      "filesystem": {
        "content_path": "/tmp/bazel_cache/ac",
        "temp_path": "/tmp/bazel_cache/tmp_data",
        "eviction_policy": {
          // 100mb.
          "max_bytes": 100000000,
        }
      }
    }
  ],
  // Place rest of configuration here ...
}
```

### S3 Store

Since Amazon's S3 service now has strong consistency, it's very reliable to use
as a back end of a CAS. This pairs well with the `compression` and `dedup`
stores, but in this example we'll store the raw files.

```js
{
  "stores": [
    {
      "name": "CAS_MAIN_STORE",
      "experimental_s3_store": {
        // Region the bucket lives in.
        "region": "us-west-1",
        // Name of the bucket to upload to.
        "bucket": "some-bucket-name",
        // Adds an optional prefix to objects before uploaded.
        "key_prefix": "cas/",
        // S3 supports retry capability.
        "retry": {
          "max_retries": 6,
          "delay": 0.3,
          "jitter": 0.5,
        }
      }
    },
    {
      "name": "AC_MAIN_STORE",
      "experimental_s3_store": {
        "region": "us-west-1",
        "bucket": "some-bucket-name",
        "key_prefix": "ac/",
        "retry": {
          "max_retries": 6,
          "delay": 0.3,
          "jitter": 0.5,
        }
      }
    }
  ],
  // Place rest of configuration here ...
}
```

### Fast Slow Store

This store will first attempt to read from the `fast` store when reading and if
it does exist return it. If it doesn't exist, try to fetch it from the `slow`
store and while streaming it to the client also populate the `fast` store with
the requested object. When transferring (uploading) from client, the data will
be placed into both `fast` and `slow` stores simultaneously.

In this example, we'll hold about 1gb of frequently accessed data in memory and
the rest will be stored in AWS's S3:

```js
{
  "stores": [
    {
      "name": "CAS_MAIN_STORE",
      "fast_slow": {
        "fast": {
          "memory": {
            "eviction_policy": {
              // 1gb.
              "max_bytes": 1000000000,
            }
          }
        },
        "slow": {
          "experimental_s3_store": {
            "region": "us-west-1",
            "bucket": "some-bucket-name",
            "key_prefix": "cas/",
          }
        }
      }
    },
    {
      "name": "AC_MAIN_STORE",
      "fast_slow": {
        "fast": {
          "memory": {
            "eviction_policy": {
              // 100mb.
              "max_bytes": 100000000,
            }
          }
        },
        "slow": {
          "experimental_s3_store": {
            "region": "us-west-1",
            "bucket": "some-bucket-name",
            "key_prefix": "ac/",
          }
        }
      }
    }
  ],
  // Place rest of configuration here ...
}
```

### Verify Store

This store is special. It's only job is to verify the content as it's fetched
and uploaded to ensure it meets some criteria or errors. This store should only
be added to the CAS. If `hash_verification_function` is set, it will apply the
hashing algorithm on the data as it's sent/received and at the end if it
doesn't not match the name of the digest it will cancel the upload/download and
return an error. If it's not set, the hashing verification will be disabled.
If `verify_size` is set, a similar item will happen, but count the bytes sent
and check it against the digest instead.

```js
{
  "stores": [
    {
      "name": "CAS_MAIN_STORE",
      "verify": {
        "backend": {
          "memory": {
            "eviction_policy": {
              // 1gb.
              "max_bytes": 1000000000,
            }
          }
        },
        "verify_size": true,
        // sha256 or blake3
        "hash_verification_function": "sha256",
      }
    },
    {
      "name": "AC_MAIN_STORE",
      "memory": {
        "eviction_policy": {
          // 100mb.
          "max_bytes": 100000000,
        }
      }
    }
  ],
  // Place rest of configuration here ...
}
```

<img referrerpolicy="no-referrer-when-downgrade" src="https://nativelink.matomo.cloud/matomo.php?idsite=2&amp;rec=1&amp;action_name=nativelink-config%20Readme.md" style="border:0" alt="" />
