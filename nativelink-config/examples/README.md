NativeLink uses a JSON file as the configuration format. This section of the
documentation will breakdown the anatomy of this configuration file with step-by-step instructions on its assembly.

# Assembling the Configuration File

Configuration JSON objects should begin with the `stores` key, followed by `workers`, `schedulers`, `servers`, and `global`.


```json5
{
  "stores": [],
  "workers": [],
  "schedulers": [],
  "servers": [],
  "global": {}
}
```

This is the scaffolding for a NativeLink deployment configuration.


<details>
  <summary>Configuring Store</summary>

### Store Name

The value of `stores` is an array where each element defines a store. Each should have a user supplied name. The following example, defines the `AC_MAIN_STORE`.

```json5
{
  "stores": [
    {
      "name": "AC_MAIN_STORE",
    }
  ],
  "workers": [],
  "schedulers": [],
  "servers": [],
  "global": {},
}
```

### Store Type

Once the store has been named and its object exists,
the next key to add is the type of store. The options are `filesystem`, `memory`, `compression`, `dedup`, `fast_slow`, `verify`, and `experimental_cloud_object_store`.

```json5
{
  "stores": [
    {
      "name": "AC_MAIN_STORE",
      "filesystem": {}
    }
  ],
  "workers": [],
  "schedulers": [],
  "servers": [],
  "global": {},
}
```

### Store Options

The contents of the object here must include `content_path`, `temp_path`, and an embedded JSON object, `eviction_policy`, which specifies the value of `max_bytes` for the store.

```json5
{
  "stores": [
    {
      "name": "AC_MAIN_STORE",
      "filesystem": {
        "content_path": "/tmp/nativelink/data/content_path-index",
        "temp_path": "/tmp/nativelink/data/tmp_path-index",
        "eviction_policy": {
          // 500mb.
          "max_bytes": 500000000,
        }
      }
    }
  ],
  "workers": [],
  "schedulers": [],
  "servers": [],
  "global": {},
}
```
</details>


<details>
  <summary>Configuring Workers </summary>

### Worker Array

The value of `workers` includes a top level array that embeds the worker metadata. This array always begins with the `local` object, which is the only item permitted at this time.

```json5
{
  "stores": [],
  "workers": [{
    "local": {}
  }],
  "schedulers": [],
  "servers": [],
  "global": {},
}
```

### Local Worker Object Members

The Local object has five components, `worker_api_endpoint`, `cas_fast_slow_store`, `upload_action_results`,`work_directory`, and `platform_properties`.

```json5
{
  "stores": [],
  "workers": [{
    "local": {
      "worker_api_endpoint": {
        "uri": "grpc://127.0.0.1:50061",
      },
      "cas_fast_slow_store": "WORKER_FAST_SLOW_STORE",
      "upload_action_result": {
        "ac_store": "AC_MAIN_STORE",
      },
      "work_directory": "/tmp/nativelink/work",
      "platform_properties": {
        "cpu_count": {
          "values": ["16"],
        },
        "memory_kb": {
          "values": ["500000"],
        },
        "network_kbps": {
          "values": ["100000"],
        },
        "cpu_arch": {
          "values": ["x86_64"],
        },
        "OSFamily": {
          "values": [""]
        },
        "container-image": {
          "values": [""]
        },
      }
    }
  }],
  "schedulers": [],
  "servers": [],
  "global": {},
}
```
</details>


<details>
  <summary>Configuring Schedulers </summary>

### Scheduler Name
The value of `stores` is an array where each element defines a store. Each should have a user supplied name. The following example, defines the `AC_MAIN_STORE`.

The value of `schedulers` is an array where each element defines a scheduler. Each scheduler must have a user-supplied name. The following example, defines the `MAIN_SCHEDULER`.

```json5
{
  "stores": [],
  "workers": [],
  "schedulers": [
    {
      name: "MAIN_SCHEDULER"
    }
  ],
  "servers": [],
  "global": {},
}
```

### Scheduler Type

Once the scheduler has been named and its object exists,
the next key is the type of scheduler. The options are `simple`, `action_scheduler`, `grpc_scheduler`, `property_modifier_scheduler`, and `worker_scheduler`.

```json5
{
  "stores": [],
  "workers": [],
  "schedulers": [
    {
      name: "MAIN_SCHEDULER",
      simple: {}
    }
  ],
}
```

### Scheduler Options

The contents of the scheduler type object defines the options. For a list of options see the documentation. See the example below.

```json5
{
  "stores": [],
  "workers": [],
  "schedulers": [
    {
      name: "MAIN_SCHEDULER",
      simple: {
        "supported_platform_properties": {
          "cpu_count": "minimum",
          "memory_kb": "minimum",
          "network_kbps": "minimum",
          "disk_read_iops": "minimum",
          "disk_read_bps": "minimum",
          "disk_write_iops": "minimum",
          "disk_write_bps": "minimum",
          "shm_size": "minimum",
          "gpu_count": "minimum",
          "gpu_model": "exact",
          "cpu_vendor": "exact",
          "cpu_arch": "exact",
          "cpu_model": "exact",
          "kernel_version": "exact",
          "OSFamily": "priority",
          "container-image": "priority",
        }
      }
    }
  ],
  "servers": [],
  "global": {},
}
```

</details>

<details>
  <summary>Configuring Servers</summary>

### Servers

The `servers` configuration object is an array, with two objects, `public`, and `private_workers_servers`.

```json5
{
  "stores": [],
  "workers": [],
  "schedulers": [],
  "servers": [{
    "name": "public"
  },{
    "name": "private_workers_servers"
  }],
  "global": {},
}
```

### Public Server

The `public` server consists of a `listener` object and a `services` object. The `listener` object is one level of depth and includes an `http` with a `socket address`. The `services` server consists of a `cas`, an `ac`, the `execution`, `capabilities`, and `bytestream`.

```json5
{
  "stores": [],
  "workers": [],
  "schedulers": [],
  "servers": [{
    "name": "public",
    "listener": {
      "http": {
        "socket_address": "0.0.0.0:50051"
      }
    },
    "services": {
      "cas": [{
        "instance_name": "main",
        "cas_store": "WORKER_FAST_SLOW_STORE"
      }],
      "ac": [{
        "instance_name": "main",
        "ac_store": "AC_MAIN_STORE"
      }],
      "execution": [{
        "instance_name": "main",
        "cas_store": "WORKER_FAST_SLOW_STORE",
        "scheduler": "MAIN_SCHEDULER",
      }],
      "capabilities": [{
        "instance_name": "main",
        "remote_execution": {
          "scheduler": "MAIN_SCHEDULER",
        }
      }],
      "bytestream": [{
        "instance_name": "main",
        "cas_store": "WORKER_FAST_SLOW_STORE",
      }]
    },
  },{
    "name": "private_workers_servers"
  }],
  "global": {},
}
```

### Private Server

> ⚠️ _WARNING_: A private server shouldn't be exposed to the public. ⚠️

The `private` server consists of a `listener` object and a `services` object. The `listener` object is one level and includes an `http` with a `socket address`. The `services` server consists of a `worker_api` object with `scheduler_field` and an `admin` object.

```json5
 {
  "stores": [],
  "workers": [],
  "schedulers": [],
  "servers": [{
    "name": "public",
    "listener": {
      "http": {
        "socket_address": "0.0.0.0:50051"
      }
    },
    "services": {
      "cas": [{
        "instance_name": "main",
        "cas_store": "WORKER_FAST_SLOW_STORE"
      }],
      "ac": [{
        "instance_name": "main",
        "ac_store": "AC_MAIN_STORE"
      }],
      "execution": [{
        "instance_name": "main",
        "cas_store": "WORKER_FAST_SLOW_STORE",
        "scheduler": "MAIN_SCHEDULER",
      }],
      "capabilities": [{
        "instance_name": "main",
        "remote_execution": {
          "scheduler": "MAIN_SCHEDULER",
        }
      }],
      "bytestream": [{
        "instance_name": "main",
        "cas_store": "WORKER_FAST_SLOW_STORE",
      }]
    },
  },{
    "name": "private_workers_servers",
    "listener": {
      "http": {
        "socket_address": "0.0.0.0:50061"
      }
    },
    "services": {
      // Note: This should be served on a different port, because it has
      // a different permission set than the other services.
      // In other words, this service is a backend api. The ones above
      // are a frontend api.
      "worker_api": {
        "scheduler": "MAIN_SCHEDULER",
      },
      "admin": {},
      "health": {
        "timeout_seconds": 2
      },
    }
  }],
  "global": {},
}
```

*`global`* is a single-level object and can be added at the end as the configuration object for file descriptors.

```json5
 "global": {
    "max_open_files": 24576
  }
```

</details>

<details>
  <summary>Complete Example </summary>

Below, you will find a fully tested example that you can also find in [basic_cas.json](basic_cas.json)

```json5

{
  "stores": [
    {
      name: "AC_MAIN_STORE",
      "filesystem": {
        "content_path": "/tmp/nativelink/data-worker-test/content_path-ac",
        "temp_path": "/tmp/nativelink/data-worker-test/tmp_path-ac",
        "eviction_policy": {
          // 1gb.
          "max_bytes": 1000000000,
        }
      }
    },
    {
      name: "WORKER_FAST_SLOW_STORE",
      "fast_slow": {
        // "fast" must be a "filesystem" store because the worker uses it to make
        // hardlinks on disk to a directory where the jobs are running.
        "fast": {
          "filesystem": {
            "content_path": "/tmp/nativelink/data-worker-test/content_path-cas",
            "temp_path": "/tmp/nativelink/data-worker-test/tmp_path-cas",
            "eviction_policy": {
              // 10gb.
              "max_bytes": 10000000000,
            }
          }
        },
        "slow": {
          /// Discard data.
          /// This example usage has the CAS and the Worker live in the same place,
          /// so they share the same underlying CAS. Since workers require a fast_slow
          /// store, we use the fast store as our primary data store, and the slow store
          /// is just a noop, since there's no shared storage in this config.
          "noop": {}
        }
      }
    }
  ],
  "schedulers": [
    {
      name: "MAIN_SCHEDULER",
      "simple": {
        "supported_platform_properties": {
          "cpu_count": "minimum",
          "memory_kb": "minimum",
          "network_kbps": "minimum",
          "disk_read_iops": "minimum",
          "disk_read_bps": "minimum",
          "disk_write_iops": "minimum",
          "disk_write_bps": "minimum",
          "shm_size": "minimum",
          "gpu_count": "minimum",
          "gpu_model": "exact",
          "cpu_vendor": "exact",
          "cpu_arch": "exact",
          "cpu_model": "exact",
          "kernel_version": "exact",
          "OSFamily": "priority",
          "container-image": "priority",
          // Example of how to set which docker images are available and set
          // them in the platform properties.
          // "docker_image": "priority",
        }
      }
    }
  ],
  "workers": [{
    "local": {
      "worker_api_endpoint": {
        "uri": "grpc://127.0.0.1:50061",
      },
      "cas_fast_slow_store": "WORKER_FAST_SLOW_STORE",
      "upload_action_result": {
        "ac_store": "AC_MAIN_STORE",
      },
      "work_directory": "/tmp/nativelink/work",
      "platform_properties": {
        "cpu_count": {
          "values": ["16"],
        },
        "memory_kb": {
          "values": ["500000"],
        },
        "network_kbps": {
          "values": ["100000"],
        },
        "cpu_arch": {
          "values": ["x86_64"],
        },
        "OSFamily": {
          "values": [""]
        },
        "container-image": {
          "values": [""]
        },
        // Example of how to set which docker images are available and set
        // them in the platform properties.
        // "docker_image": {
        //   "query_cmd": "docker images --format {{.Repository}}:{{.Tag}}",
        // }
      }
    }
  }],
  "servers": [{
    "name": "public",
    "listener": {
      "http": {
        "socket_address": "0.0.0.0:50051"
      }
    },
    "services": {
      "cas": [{
        "instance_name": "main",
        "cas_store": "WORKER_FAST_SLOW_STORE"
      }],
      "ac": [{
        "instance_name": "main",
        "ac_store": "AC_MAIN_STORE"
      }],
      "execution": [{
        "instance_name": "main",
        "cas_store": "WORKER_FAST_SLOW_STORE",
        "scheduler": "MAIN_SCHEDULER",
      }],
      "capabilities": [{
        "instance_name": "main",
        "remote_execution": {
          "scheduler": "MAIN_SCHEDULER",
        }
      }],
      "bytestream": [{
        "instance_name": "main",
        "cas_store": "WORKER_FAST_SLOW_STORE",
      }]
    }
  }, {
    "name": "private_workers_servers",
    "listener": {
      "http": {
        "socket_address": "0.0.0.0:50061"
      }
    },
    "services": {
      // Note: This should be served on a different port, because it has
      // a different permission set than the other services.
      // In other words, this service is a backend api. The ones above
      // are a frontend api.
      "worker_api": {
        "scheduler": "MAIN_SCHEDULER",
      },
      "admin": {}
    }
  }],
  "global": {
    "max_open_files": 24576
  }
}
```

</details>

<details>
  <summary>High-Performance tmpfs Configuration</summary>

### Using tmpfs for Maximum I/O Performance

NativeLink uses hardlinks to efficiently set up action sandboxes from the CAS filesystem store.
This requires the `work_directory` and the CAS `content_path` to be on the **same filesystem**.

For maximum I/O performance, you can place both on a tmpfs (RAM-based filesystem).
Users have reported **3-4x build time improvements** when using tmpfs.

#### Setup Instructions

1. Create a tmpfs mount point:
```bash
sudo mkdir -p /mnt/tmpfs/nativelink
sudo mount -t tmpfs -o size=50G tmpfs /mnt/tmpfs/nativelink
```

2. To make it persistent across reboots, add to `/etc/fstab`:
```
tmpfs /mnt/tmpfs/nativelink tmpfs size=50G,mode=1777 0 0
```

3. Configure NativeLink to use the tmpfs paths. See [tmpfs-worker.json5](tmpfs-worker.json5) for a complete example.

#### Key Configuration Points

Both paths must be on the same tmpfs mount:
- CAS `content_path`: `/mnt/tmpfs/nativelink/cas`
- Worker `work_directory`: `/mnt/tmpfs/nativelink/work`

#### Trade-offs

| Pros | Cons |
|------|------|
| Maximum I/O performance (RAM speed) | Cache is lost on restart/reboot |
| Hardlinks work correctly (same filesystem) | Limited by available RAM |
| Eliminates disk I/O bottleneck | Not suitable for very large CAS |

#### Sizing Guidance

- Set tmpfs size to ~50% of available RAM
- Set `max_bytes` in eviction_policy to ~80% of tmpfs size
- Monitor memory usage and adjust as needed

</details>

<img referrerpolicy="no-referrer-when-downgrade" src="https://nativelink.matomo.cloud/matomo.php?idsite=2&amp;rec=1&amp;action_name=nativelink-config%20examples%20Readme.md" style="border:0" alt="" />
