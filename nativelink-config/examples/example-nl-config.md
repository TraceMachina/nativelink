NativeLink uses a JSON file as the configuration format. This section of the
documentation will breakdown the anatomy of this configuration file with step-by-step instructions on its assembly.

# Assembling the `cas.json` Config File

Configuration JSON objects should begin with the `"stores"` key, followed by `"workers"`, `"schedulers"`, or other services you may wish to instantiate.


```json
{
  "stores": {},
  "workers": {},
  "schedulers": {},
}
```

This is the scaffolding for a NativeLink deployment configuration.


<details>
  <summary>Configuring Store</summary>

### Store Name

The value of `stores` includes top-level keys, which are the names of stores. The following example, defines the `AC_MAIN_STORE`.

```json
{
  "stores": {
    "AC_MAIN_STORE": {}
  },
  "workers": {},
  "schedulers": {},
}
```

### Store Type

Once the store has been named and its object exists,
the next key is the type of store. The options are `filesystem`, `memory`, `compression`, `dedup`, `fast_slow`, `verify`, and `experimental_s3_store`.

```json
{
  "stores": {
    "AC_MAIN_STORE": {
        "filesystem": {}
    }
  },
  "workers": {},
  "schedulers": {},
}
```

### Store Options

The contents of the object here must include `content_path`, `temp_path`, and an embedded JSON object, `eviction_policy`, which specifies the value of `max_bytes` for the store.

```json
{
  "stores": {
    "AC_MAIN_STORE": {
      "filesystem": {
        "content_path": "/tmp/nativelink/data/content_path-index",
        "temp_path": "/tmp/nativelink/data/tmp_path-index",
        "eviction_policy": {
          // 500mb.
          "max_bytes": 500000000,
        }
      }
    }
  },
  "workers": {},
  "schedulers": {},
}
```
</details>


<details>
  <summary>Configuring Workers </summary>

  ## TODO

</details>


<details>
  <summary>Configuring Schedulers </summary>

  ## TODO

</details>
