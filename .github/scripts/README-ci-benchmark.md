# NativeLink cluster CI

**CI / NativeLink** runs `bazel test //...` on the retained cluster client.
GitHub's hosted job checks out the dispatch helpers, obtains short-lived
credentials, dispatches the workload and publishes aggregate results. Nix shell
preparation, repository downloads, the action disk cache, expanded repositories,
Bazel outputs and the running JVM stay on the cluster.

Each repository branch push runs once by default. Manual runs accept 1–20
iterations for cache and timing checks. A failure stops subsequent iterations
and preserves its exit code. This workflow runs one cluster job without a hosted comparison or matrix. The earlier comparison reader remains available for historical
aggregate reports; CI execution accepts only the cluster lane.

The hosted Nix Bazel Dev job and the duplicate project-pinned native Bazel job
are replaced by this lane. Bazel 8 compatibility, Redis topology, sanitizer,
Cargo/Windows, Nix toolchain/installation, integration and release checks retain
their separate coverage. Their workloads are not equivalent to `test //...`.

Both the Bazel version and dependencies come from the tested commit. The first
run after a toolchain change may populate new cache entries. Changed inputs
invalidate affected actions normally; test-result caching stays enabled. A
client pod replacement loses JVM memory but preserves its two existing persistent volumes.

The Nix shell supplies a C++ runtime for native Node modules. The benchmark
removes that library-path override only when launching host Bazel, which must
use its own runtime. Other Nix shell commands keep their normal environment.

## Configuration and trust

The workflow file remains `parallel-cluster-ci.yaml` because the federated
identity is bound to that exact path. Its environment remains
`cluster-ci-benchmark`. Allow repository **branches** with the `**/*` deployment
branch pattern, not tag policies. The identity provider must require the exact
repository and owner IDs, this workflow path at the signed branch ref, a
`refs/heads/` ref, a push/manual event and this environment subject.

Only users or integrations allowed to push to the source repository can start
this cluster lane. External fork PRs do not receive its environment secrets or
retained client. After reviewing an external contribution, a maintainer can
promote its exact commit to a repository branch such as `ci-approved/pr-123`;
that push runs the same cluster CI against the reviewed commit. Do not promote
unreviewed changes. The build workflow uses push/manual events.

The repository's Actions approval policy is `all_external_contributors`.
Ordinary fork PR workflows from all outside contributors require approval from
someone with write access, including contributors with previously merged
changes. Repository and organization members can run automatically. Use
GitHub's **Approve and run** control after reviewing the proposed changes.
This repository setting is separate from the cluster's branch restriction:

```sh
gh api repos/TraceMachina/nativelink/actions/permissions/fork-pr-contributor-approval \
  --method PUT -f approval_policy=all_external_contributors
gh api repos/TraceMachina/nativelink/actions/permissions/fork-pr-contributor-approval \
  -H 'Cache-Control: no-cache' --jq '.approval_policy'
```

See [GitHub's approval policy documentation](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository#controlling-changes-from-forks-to-workflows-in-public-repositories).
Existing metadata checks using `pull_request_target` run in the base repository
context and must never check out or execute fork code.

Set `NATIVELINK_CLUSTER_API_KEY` in the environment, using a dedicated build key
with cache read/write and BEP publishing, without worker registration. Configure:

| Variable | Meaning |
| --- | --- |
| `NATIVELINK_CLUSTER_ENDPOINT` | TLS REAPI endpoint, `grpcs://host:443` |
| `NATIVELINK_CLUSTER_BEP_ENDPOINT` | TLS BEP endpoint |
| `NATIVELINK_CLUSTER_INSTANCE` | Remote instance name |
| `NATIVELINK_CLUSTER_RESULTS_URL` | HTTPS build results URL prefix |
| `NATIVELINK_CLUSTER_WORKER_IMAGE` | Exact image property advertised by the worker pool |

The cluster lane reserves 1,000 CPU units, 2,000,000 KB memory and 1,000,000 KB
disk per action, with at most 48 concurrent jobs. Those values must fit the
configured pool. Missing credentials stop the build without anonymous fallback.
The host/port-scoped credential helper reads the key from its environment.
No key is written to a configuration file, passed in arguments or added to build metadata.
Do not enable shell tracing or upload raw BEP, logs, profiles or environments.

## Retained client access

Configure `NL_GCP_PROVIDER`, `NL_GCP_SERVICE_ACCOUNT`, `NL_GKE_SERVER`,
`NL_GKE_CA_DATA`, `NL_GKE_NAMESPACE` and `NL_GKE_POD` as environment variables.
The API address and public CA are configuration, not credentials. GitHub OIDC
exchanges the job identity for a short-lived token in memory. Include the
`userinfo.email` scope for the GKE email-based RoleBinding.

The role permits get/exec on one named client pod, with no Secret reads or
exec access to database/worker pods. No service-account key or developer
cluster credentials are distributed. A temporary cluster configuration contains only connection
settings and the exec-plugin path. Selected build variables travel over
encrypted stdin; GitHub and OIDC credentials are not forwarded to the client.
The checkout rejects tracked edits before changing to the exact source commit;
a file lock serializes writers and a bounded timeout limits abandoned work.

The timeout test distinguishes a terminated zombie from a live child. That
expectation issue was discovered during a soak test in our systems; host reaping
timing must not decide whether the worker terminated the process.

## Reading results

The Actions summary reports source, setup plus workload time, each iteration's
build/test duration, exit status and cache/test counts. The JSON artifact contains
only constructed aggregate fields. It excludes raw events, credentials and
arbitrary server messages. Build and test time is combined because `bazel test`
builds its prerequisites in the same invocation. Queueing is excluded.

Local action, disk and remote hits are reconciled as:

`(local action hits + disk hits + remote hits) / (local action hits + process outcomes)`

Process outcomes exclude `total`, `internal` and duplicated `action cache hit`
runner counters. Missing local coverage yields an unknown total. Cached test
results overlap action reuse and remain separate. These definitions follow
[Bazel's SpawnStats](https://github.com/bazelbuild/bazel/blob/9.1.1/src/main/java/com/google/devtools/build/lib/runtime/SpawnStats.java).
