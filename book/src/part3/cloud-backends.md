# Cloud Backends

`NativeLink` can back CAS and AC with cloud object storage through a single
store type: `experimental_cloud_object_store`. Six providers ship today — AWS
S3, Google Cloud Storage, Azure Blob, Cloudflare R2, Oracle Cloud
Infrastructure (OCI) Object Storage, and `NetApp` ONTAP S3. This chapter covers
the provider-tagged schema, the fields each provider actually accepts, and when
to reach for which.

## The Provider-Tagged Interface

`experimental_cloud_object_store` is not one uniform backend behind a config
map. It is a `#[serde(tag = "provider")]` enum: every block MUST carry a
`provider` field, and the remaining fields are provider-specific. A field that
belongs to one provider is rejected on another, because every variant is
`#[serde(deny_unknown_fields)]`.

**Source:** [`ExperimentalCloudObjectSpec`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/src/stores.rs) — `nativelink-config/src/stores.rs:1070-1080`

```rust
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ExperimentalCloudObjectSpec {
    Aws(ExperimentalAwsSpec),
    Gcs(ExperimentalGcsSpec),
    Azure(ExperimentalAzureSpec),
    Ontap(ExperimentalOntapS3Spec),
    R2(ExperimentalR2Spec),
    Oci(ExperimentalOciSpec),
}
```

The `provider` values are the `snake_case` variant names: `"aws"`, `"gcs"`,
`"azure"`, `"ontap"`, `"r2"`, `"oci"`. There is no default provider in
practice — omit `provider` and the config fails to parse. (The Rust `Default`
impl picks AWS, but that path is only reached by internal code that never round
trips through JSON5; `stores.rs:1082-1086`.)

Despite the `experimental_` prefix — a naming holdover, not a stability
warning — these backends carry production CAS/AC traffic.

### Three implementations, not one

The chapter's job is to be honest about what runs under each variant, because
the tuning knobs differ. The store factory dispatches each variant to a
distinct implementation (`nativelink-store/src/default_store_factory.rs:65-84`):

| `provider` | Backend crate / client | Notes |
|------------|------------------------|-------|
| `aws` | `aws_sdk_s3` (`S3Store`) | The canonical S3 path. |
| `r2` | `aws_sdk_s3` via `R2Store` | Thin adapter: points the S3 SDK at an R2 endpoint. |
| `oci` | `aws_sdk_s3` via `OciStore` | Thin adapter: S3-compatibility endpoint, path-style. |
| `ontap` | `aws_sdk_s3` via `OntapS3Store` | S3-compatible ONTAP endpoint plus custom TLS roots. |
| `gcs` | hand-rolled `gcs_client` over `reqwest` | Not the S3 SDK; native GCS JSON API. |
| `azure` | `azure_storage_blobs` SDK | Blob containers, not S3 buckets. |

There is no Apache Arrow `object_store` layer and no `additional_config`
pass-through map. `r2`, `oci`, and `ontap` are config adapters that construct an
S3 SDK client pointed at a non-AWS endpoint and hand it to `S3Store`
(`r2_store.rs:28`, `oci_store.rs:29`, `ontap_s3_store.rs`), so they inherit S3's
multipart upload and retry behavior verbatim.

### TLS

The S3-family path (`aws` / `r2` / `oci` / `ontap`) builds its HTTPS connector with
`hyper-rustls` and the platform certificate verifier
(`common_s3_utils.rs:65`), so system trust roots are used with no extra
configuration. Setting `insecure_allow_http: true` downgrades the connector
from `https_only` to `https_or_http` (`common_s3_utils.rs:67-71`) — intended for
local testing against a plaintext S3 emulator only. `disable_http2: true` drops
HTTP/2 from the connector for environments where it misbehaves.

## Fields Shared Across Providers

Every provider variant flattens a `CommonObjectSpec`
(`stores.rs:1168-1230`), so these fields sit at the same level as the
provider-specific ones:

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `key_prefix` | string | none | Namespace within the bucket/container. Separate CAS from AC, or environments from each other. |
| `retry` | `Retry` | see below | Exponential back-off with jitter. |
| `consider_expired_after_s` | seconds | `0` (never) | Treat objects older than this as absent, so an external lifecycle tool can delete cold data and clients re-upload. |
| `max_retry_buffer_per_request` | bytes | 5 MB | Buffer retained to retry a failed upload. `0` disables upload buffering. |
| `multipart_max_concurrent_uploads` | count | `10` | Parallel `UploadPart` requests per multipart upload. Higher is faster and uses more memory. |
| `insecure_allow_http` | `bool` | `false` | Allow plaintext HTTP. Local testing only. |
| `disable_http2` | `bool` | `false` | Force HTTP/1.1. |

`retry` is the standard `NativeLink` `Retry` struct (`stores.rs:1586-1623`):
`max_retries`, `delay` (seconds, base for exponential back-off), `jitter`
(fractional), and an optional `retry_on_errors` code list.

### Multipart sizing and memory bounds

Above ~5 MiB, `S3Store` uploads via multipart. Parts are sized at a **64 MiB
target** (`TARGET_MULTIPART_PART_SIZE`), grown only as needed to keep the count
within S3's 10,000-part ceiling for very large objects — *not* at the 5 MiB
floor. Sizing at the floor turns a multi-GiB blob into thousands of tiny
concurrent `PUT`s, which churns and poisons the HTTP connection pool
(`aws-smithy` "connection never set" storms) and makes large writes slow and
flaky; on a lossy link the retries exhaust and the write fails outright. A
12 GiB blob is ~192 parts, not ~2300.

Two limits bound memory. `multipart_max_concurrent_uploads` (default 10) caps
parallel parts **per upload** — peak per-upload footprint is roughly that count
times the part size. A store-wide semaphore (`MAX_CONCURRENT_MULTIPART_UPLOADS`,
4) then caps how many multipart uploads run **at once** across the whole store,
so a burst of concurrent large writes can't multiply the per-upload footprint
into an out-of-memory on a cache node. The store-wide permit is admission
control — held for the whole upload, never per part — so it can't deadlock the
inner part loop; a fifth concurrent large upload simply waits for a slot.

## AWS S3 (`provider: "aws"`)

`ExperimentalAwsSpec` (`stores.rs:1088-1103`) adds only `region` and `bucket`
on top of the common fields.

```json5
{
  name: "S3_CAS",
  experimental_cloud_object_store: {
    provider: "aws",
    region: "us-east-1",
    bucket: "my-nativelink-cas",
    key_prefix: "cas/",
    retry: { max_retries: 5, delay: 0.1, jitter: 0.5 },
    multipart_max_concurrent_uploads: 10,
  },
}
```

Authentication uses the standard AWS credential chain (environment variables,
instance profile, ECS task role, IMDS). There is no credential field in the
spec — put nothing secret in the config.

## Cloudflare R2 (`provider: "r2"`)

R2 has a first-class adapter (`r2_store.rs`) so you do not have to hand-assemble
the generic S3 endpoint. R2 is S3-compatible with no egress fees, which is
compelling for remote caching — the dominant cost of a cache is downloading
artifacts on a hit, and R2 charges nothing for that.

`ExperimentalR2Spec` (`stores.rs:723-748`) takes `account_id` (the endpoint is
derived as `https://{account_id}.r2.cloudflarestorage.com`,
`r2_store.rs:91-92`), `bucket`, and optional explicit `access_key_id` /
`secret_access_key`.

```json5
{
  name: "R2_CAS",
  experimental_cloud_object_store: {
    provider: "r2",
    account_id: "636b76d9ace55d0aacba7980073c7ca7",
    bucket: "my-nativelink-cas",
    access_key_id: "${R2_ACCESS_KEY_ID}",
    secret_access_key: "${R2_SECRET_ACCESS_KEY}",
    key_prefix: "cas/",
    retry: { max_retries: 6, delay: 0.3, jitter: 0.5 },
  },
}
```

The `${...}` syntax is shell expansion — `NativeLink` expands environment
variables in string config values, so keep the keys in the environment rather
than the file. If you omit both keys, the store falls back to the AWS default
credential chain, but that chain only reads `AWS_*` environment variable names,
so your R2 keys would have to live under `AWS_ACCESS_KEY_ID` /
`AWS_SECRET_ACCESS_KEY` for the fallback to find them.

## Google Cloud Storage (`provider: "gcs"`)

GCS does not go through the S3 SDK — it uses a hand-rolled client over `reqwest`
(`gcs_client/client.rs:126`) that speaks the native GCS JSON API.
`ExperimentalGcsSpec` (`stores.rs:1105-1139`) therefore has **no `region`
field**; it takes `bucket` plus a few GCS-specific knobs.

```json5
{
  name: "GCS_CAS",
  experimental_cloud_object_store: {
    provider: "gcs",
    bucket: "my-nativelink-cas",
    key_prefix: "cas/",
    resumable_chunk_size: "8mb",
    authentication_required: true,
    connection_timeout_s: 3,
    read_timeout_s: 3,
    retry: { max_retries: 6, delay: 0.3, jitter: 0.5 },
    multipart_max_concurrent_uploads: 10,
  },
}
```

Authentication uses Application Default Credentials — a service account JSON key
via `GOOGLE_APPLICATION_CREDENTIALS`, or workload identity on GKE. Set
`authentication_required: true` to fail fast when no credentials are found
instead of falling back to anonymous access. `resumable_chunk_size` (default
2 MB) controls resumable-upload chunking.

## Azure Blob Storage (`provider: "azure"`)

Azure uses the `azure_storage_blobs` SDK (`azure_blob_store.rs:22-26`). Its
spec (`stores.rs:1141-1166`) speaks Azure's vocabulary: `account_name` and
`container`, **not** `region`/`bucket`.

```json5
{
  name: "AZURE_CAS",
  experimental_cloud_object_store: {
    provider: "azure",
    account_name: "mynativelink",
    container: "my-container",
    key_prefix: "cas/",
    connection_timeout_s: 3,
    read_timeout_s: 3,
    retry: { max_retries: 6, delay: 0.3, jitter: 0.5 },
    multipart_max_concurrent_uploads: 10,
  },
}
```

Azure credentials come from the environment, not the config file. There is no
access-key field in the spec.

## Oracle OCI Object Storage (`provider: "oci"`)

> **Namesake warning — two unrelated meanings of OCI.** Here `provider: "oci"` means
> *Oracle Cloud Infrastructure* Object Storage: a cloud **store backend** that
> holds CAS/AC blobs. It is not the *Open Container Initiative* image format.

Oracle Object Storage has its own adapter (`oci_store.rs`). It
talks to OCI's Amazon S3 Compatibility API through `aws_sdk_s3`, using path-style
addressing: the namespace is the host prefix and the bucket is the first path
segment,
`https://{namespace}.compat.objectstorage.{region}.oci.customer-oci.com/{bucket}/{object}`
(`oci_store.rs:106-110`). `ExperimentalOciSpec` (`stores.rs:750-795`) takes
`namespace`, `region`, `bucket`, and optional Customer Secret Key credentials.

```json5
{
  name: "OCI_CAS",
  experimental_cloud_object_store: {
    provider: "oci",
    namespace: "axaxnpcrorw5",
    region: "us-phoenix-1",
    bucket: "my-nativelink-cas",
    access_key_id: "${OCI_ACCESS_KEY_ID}",
    secret_access_key: "${OCI_SECRET_ACCESS_KEY}",
    key_prefix: "cas/",
    retry: { max_retries: 6, delay: 0.3, jitter: 0.5 },
  },
}
```

The `namespace` is the immutable, system-generated top-level container assigned
to your tenancy — the same string in every region. Credentials are a Customer
Secret Key (an access-key/secret-key pair generated under User Settings →
Customer secret keys), signed with AWS SigV4. Oracle does not let you retrieve a
secret key after generation, so store it out of band. As with R2, omitting both
keys falls back to the `AWS_*` credential chain.

## NetApp ONTAP S3 (`provider: "ontap"`)

ONTAP S3 is the on-premises object-storage adapter
(`ontap_s3_store.rs`), aimed at teams running `NetApp` ONTAP behind the
firewall. `ExperimentalOntapS3Spec` (`stores.rs:704-721`) takes an explicit
`endpoint`, the `vserver_name`, a `bucket`, and optional `root_certificates` for
a private certificate authority.

```json5
{
  name: "ONTAP_CAS",
  experimental_cloud_object_store: {
    provider: "ontap",
    endpoint: "https://ontap-s3-endpoint:443",
    vserver_name: "my-vserver",
    bucket: "my-bucket",
    root_certificates: "/etc/nativelink/ontap-ca.pem",
    key_prefix: "cas/",
    retry: { max_retries: 6, delay: 0.3, jitter: 0.5 },
    multipart_max_concurrent_uploads: 10,
  },
}
```

ONTAP reads credentials from the AWS environment variables
(`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_DEFAULT_REGION`), not from
the config.

### The ONTAP existence-cache companion

ONTAP pairs with a separate store type, `ontap_s3_existence_cache`
(`OntapS3ExistenceCacheSpec`, `stores.rs:797-806`), which keeps an on-disk index
of known object digests to short-circuit repeated existence checks against the
filer. It is its own `StoreSpec` variant, not a field of the cloud object store,
and it wraps an ONTAP backend directly:

```json5
{
  name: "ONTAP_FAST_EXISTENCE",
  ontap_s3_existence_cache: {
    index_path: "/var/lib/nativelink/ontap-index.json",
    sync_interval_seconds: 300,
    backend: {
      endpoint: "https://ontap-s3-endpoint:443",
      vserver_name: "my-vserver",
      bucket: "my-bucket",
      key_prefix: "cas/",
    },
  },
}
```

Note that the inner `backend` is a raw ONTAP spec with **no** `provider` tag —
it is typed as `ExperimentalOntapS3Spec` directly, so adding `provider: "ontap"`
there fails the parse.

## When to Use Which

| Backend | Best For | Tradeoff |
|---------|----------|----------|
| **S3** (`aws`) | AWS-native deployments, high durability | Egress costs at scale |
| **R2** (`r2`) | Cost-sensitive, high-egress workloads | Slightly higher in-region latency than S3 |
| **GCS** (`gcs`) | GCP-native deployments | Native JSON-API client, not S3 SDK |
| **Azure** (`azure`) | Azure-native deployments | Distinct `account_name`/`container` vocabulary |
| **OCI** (`oci`) | Oracle Cloud tenancies | S3-compatibility API, path-style only |
| **ONTAP** (`ontap`) | On-premises `NetApp` filers | Self-managed durability and TLS roots |

Filesystem and memory stores remain the right choice for single-node,
development, and CI-runner use — see [The Store Catalog](./store-catalog.md).

## The Tiering Pattern

In production you rarely point a store spec straight at a cloud backend. You
tier it: a hot memory or filesystem tier in front, compression to cut storage
and transfer, and an existence cache to skip round-trips for
`FindMissingBlobs`.

```json5
{
  name: "TIERED_CAS",
  existence_cache: {
    backend: {
      fast_slow: {
        fast: {
          memory: {
            eviction_policy: { max_bytes: "4gb" },
          },
        },
        slow: {
          compression: {
            backend: {
              experimental_cloud_object_store: {
                provider: "aws",
                region: "us-east-1",
                bucket: "my-cas",
                key_prefix: "cas/",
              },
            },
            compression_algorithm: { lz4: {} },
          },
        },
      },
    },
    eviction_policy: { max_count: 1000000 },
  },
}
```

This composition gives you:

1. `existence_cache` — cheap `has` responses without hitting the network.
2. `fast_slow(memory, ...)` — hot artifacts served from RAM.
3. `compression` — smaller objects, less transfer (LZ4 aborts early on
   incompressible data).
4. `experimental_cloud_object_store` — durable, shared, scalable cold tier.

Swap the `provider` block for any of the six backends above; the surrounding
tiering is identical. See [Store Composition](./store-composition.md) for the
general pattern.

## Object Key Layout

`S3Store` derives each object key by concatenating `key_prefix` with the digest
string (`s3_store.rs:175-176`), and the digest renders as `{hex}-{size}`
(`common.rs:159-164`):

```
{key_prefix}{hash}-{size}
```

For example: `cas/a1b2c3d4e5f6...789-12345`. This flat namespace maps cleanly
onto object stores (which have no true directories), and the prefix lets several
logical stores share one bucket.

## Lifecycle and Eviction

Cloud object stores have no built-in eviction by access pattern — the store
"will never delete files, so you are responsible for purging old files in other
ways" (`stores.rs:88-90`). Options:

1. **Bucket lifecycle rules** — expire objects older than *N* days. Crude but
   effective. Pair it with `consider_expired_after_s` so `NativeLink` treats
   soon-to-be-deleted objects as absent and clients re-upload before the
   external tool removes them.
2. **Fast tiers with eviction** — memory or filesystem in front handles the hot
   set; cold data in the cloud persists until lifecycle cleanup.
3. **The `noop` slow store** — for CI where you want cache hits *within* a
   pipeline but no cross-pipeline persistence, use
   `fast_slow: { fast: filesystem, slow: noop }`. The slow store silently
   discards writes.
