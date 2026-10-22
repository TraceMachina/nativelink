# Parallel cluster CI benchmark

The separate **CI / Parallel cluster benchmark** workflow compares the same
commit and `bazel test //...` workload in two parallel Ubuntu 24.04 jobs. It
leaves the required CI workflows unchanged:

- **baseline** uses the existing Nix preparation, hermetic toolchain and staging
  remote cache in read-only mode, matching the current Bazel Dev PR lane.
- **cluster** uses the same Nix preparation and hermetic toolchain, with remote
  execution on the configured warm worker pool, writable remote cache, and
  GitHub snapshots of Bazel's action disk cache and repository downloads.

Both use `.bazelversion`; neither disables test-result caching. Repetitions
default to three and manual runs accept 1–20. A failed build stops that lane's
repetitions, retains its exit code and still uploads aggregate evidence. The
comparison job needs both lane reports and matching source revisions.

The first iteration is **not a cold-cache measurement**: remote caches may
already contain results and the cluster lane may restore a disk snapshot.
Later iterations keep the same hosted runner and Bazel process. This workflow
does not use the cluster's persistent Bazel/Nix client volumes or retain a hosted
runner's JVM across workflow runs. Measure a persistent client separately
before attributing a speedup to those features.

## Configuration and trust

Create the `cluster-ci-benchmark` GitHub environment and restrict deployment
branches to `main` and the exact pre-merge branch `feature/parallel-cluster-ci`.
Only trusted branches can run this credentialed workflow; there is no PR or
`pull_request_target` trigger. Remove the pre-merge branch exception after merge.

The existing repository secrets `NATIVELINK_STAGING_API_KEY_RO` and
`NATIVELINK_STAGING_CLAIM_BASE` configure the baseline. Add the environment secret
`NATIVELINK_CLUSTER_API_KEY`, using a dedicated build key with cache read/write
and BEP publishing, without worker registration permission. Set these
environment variables:

| Variable | Meaning |
| --- | --- |
| `NATIVELINK_CLUSTER_ENDPOINT` | TLS REAPI endpoint, `grpcs://host:443` |
| `NATIVELINK_CLUSTER_BEP_ENDPOINT` | TLS BEP endpoint |
| `NATIVELINK_CLUSTER_INSTANCE` | Remote instance name |
| `NATIVELINK_CLUSTER_RESULTS_URL` | HTTPS build results URL prefix |
| `NATIVELINK_CLUSTER_WORKER_IMAGE` | Exact image property advertised by the worker pool |

The cluster lane reserves 1,000 CPU units, 2,000,000 KB memory and 1,000,000 KB
disk per action, with at most 12 concurrent jobs. Those values must fit the
configured pool. A missing key stops the build without an anonymous fallback.

The credential helper reads the key from its environment and returns it only
for the configured TLS hosts and ports. No key is written to a `.bazelrc` file,
passed in command arguments, or included in build metadata. Do not enable
shell tracing, dump the environment, or upload raw BEP/log/profile files.
Rotate or revoke this dedicated key through the API and GitHub environment.
The credential-helper protocol is defined in the
[Bazel design](https://github.com/bazelbuild/proposals/blob/main/designs/2022-06-07-bazel-credential-helpers.md).

## Reading results

The Actions summary separates setup, first build/test, warm median and total
elapsed seconds. Build and test time is combined because `bazel test` builds
its prerequisites within the same invocation. Setup starts at the first job
step; runner queueing and cache-save teardown are excluded. The two lanes have
different cache backends and capacities, so this is an end-to-end comparison,
not an isolated measurement of remote-execution overhead.

Uploaded JSON contains only source/toolchain identifiers, invocation IDs,
elapsed times, exit codes, aggregate cache counters and test counts. It never
copies arbitrary event fields or server messages. Local action, disk and
remote hits are reconciled as:

`(local action hits + disk hits + remote hits) / (local action hits + process outcomes)`

Process outcomes exclude `total`, `internal`, and duplicated `action cache hit`
runner counters. One action can spawn several processes, so `total - internal`
is not a valid substitute. Missing local statistics yield an unknown overall
percentage. Cached test results overlap action reuse and remain a separate
count. These definitions match
[Bazel's SpawnStats](https://github.com/bazelbuild/bazel/blob/9.1.1/src/main/java/com/google/devtools/build/lib/runtime/SpawnStats.java).
