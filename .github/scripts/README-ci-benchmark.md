# Parallel cluster CI benchmark

The separate **CI / Parallel cluster benchmark** workflow compares the same
commit and `bazel test //...` workload in two parallel Ubuntu 24.04 jobs. It
leaves the required CI workflows unchanged:

- **baseline** uses the existing Nix preparation, hermetic toolchain and staging
  remote cache in read-only mode, matching the current Bazel Dev PR lane.
- **cluster** uses the same Nix preparation and hermetic toolchain, with remote
  execution on the configured warm worker pool and a retained client that
  preserves the Bazel JVM, action disk cache, repository downloads and Nix store.

Both use `.bazelversion`; neither disables test-result caching. Repetitions
default to three and manual runs accept 1–20. A failed build stops that lane's
repetitions, retains its exit code and still uploads aggregate evidence. The
comparison job needs both lane reports and matching source revisions.

The first iteration is **not a cold-cache measurement**: remote caches may
already contain results and the cluster lane retains persistent cache volumes.
Later iterations retain the same client and Bazel process. The cluster lane
also preserves that client state across workflow runs; the hosted baseline
starts on a fresh runner. Cache state and resource capacity differ between the
lanes, so results measure the complete CI setup.

The Nix shell supplies a C++ runtime for native Node modules. The benchmark
removes that library-path override only when launching host Bazel, which must
use its own runtime. This avoids loading newer Nix C++ libraries against an
older host `glibc`. Other Nix shell commands keep their normal environment.

## Configuration and trust

Create the `cluster-ci-benchmark` GitHub environment and restrict deployment
branches to `main` and the exact pre-merge branch `feature/parallel-cluster-ci`.
Only trusted branches can run this credentialed workflow; there is no PR or
`pull_request_target` trigger. Remove the pre-merge branch exception after merge.

The repository's Actions approval policy is `all_external_contributors`.
Fork PR workflows from all outside contributors require approval from someone
with write access, including contributors with previously merged changes.
Repository and organization members can run automatically. Use GitHub's
**Approve and run** control after reviewing the proposed changes.

This is a repository setting, not a YAML condition. Maintainers can configure
and verify it with:

```sh
gh api repos/TraceMachina/nativelink/actions/permissions/fork-pr-contributor-approval \
  --method PUT -f approval_policy=all_external_contributors
gh api repos/TraceMachina/nativelink/actions/permissions/fork-pr-contributor-approval \
  -H 'Cache-Control: no-cache' --jq '.approval_policy'
```

See [GitHub's approval policy documentation](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository#controlling-changes-from-forks-to-workflows-in-public-repositories).
Approval permits the ordinary fork PR workflows; it does not give fork code
the cluster environment, write key or retained client. Those remain restricted
to the trusted branch push/manual workflow described above. Existing metadata
checks using `pull_request_target` run in the base repository context and must
never check out or execute fork code.

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
disk per action, with at most 48 concurrent jobs. Those values must fit the
configured pool. A missing key stops the build without an anonymous fallback.

The credential helper reads the key from its environment and returns it only
for the configured TLS hosts and ports. No key is written to a `.bazelrc` file,
passed in command arguments, or included in build metadata. Do not enable
shell tracing, dump the environment, or upload raw BEP/log/profile files.
Rotate or revoke this dedicated key through the API and GitHub environment.
The credential-helper protocol is defined in the
[Bazel design](https://github.com/bazelbuild/proposals/blob/main/designs/2022-06-07-bazel-credential-helpers.md).

## Retained client access

The cluster job uses GitHub OIDC and a dedicated federated service account,
restricted to this exact workflow, repository identity, environment and trusted
branches. Its Kubernetes role permits inspecting and executing in one named
client pod; it cannot read Secrets or execute in database/worker pods. No GCP
service account key or developer cluster credentials are distributed.

Configure `NL_GCP_PROVIDER`, `NL_GCP_SERVICE_ACCOUNT`, `NL_GKE_SERVER`,
`NL_GKE_CA_DATA`, `NL_GKE_NAMESPACE`, and `NL_GKE_POD` as environment variables.
The API address and CA are configuration, not credentials. The exec plugin
exchanges the job's identity for a short-lived token in memory. The generated
cluster configuration contains only the server, CA and plugin path.

The dispatch sends the build key through encrypted stdin, never through command
arguments or a key file. It forwards only selected build variables, excluding
GitHub and OIDC credentials. The retained checkout rejects tracked edits before
switching to the exact commit, and a file lock permits only one active job. The
persistent client is for trusted serialized code, with no Kubernetes service
account token mounted. A bounded process timeout limits an abandoned dispatch.

The timeout test also distinguishes a terminated zombie from a live child.
That expectation issue was discovered during a soak test in our systems; host
reaping timing must not decide whether the worker terminated the process.

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
