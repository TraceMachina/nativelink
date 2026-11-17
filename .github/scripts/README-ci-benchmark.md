# NativeLink cluster CI

**CI / NativeLink** runs `bazel test //...` on the retained cluster client for
branch pushes and manual runs. Pull requests run the same suite with the
project-pinned Bazel version on a hosted Linux runner. Repository PRs use remote
execution; fork PRs use the public read-only cache and local execution without
cluster credentials. Both paths include Clippy and rustfmt.
For branch builds, GitHub's hosted job checks out the dispatch helpers, obtains short-lived
credentials, dispatches the workload and publishes aggregate results. Nix shell
preparation, repository downloads, the action disk cache, expanded repositories,
Bazel outputs and the running JVM stay on the cluster.

Each repository branch push runs once by default. Manual runs accept 1–20
iterations for cache and timing checks. A failure stops subsequent iterations
and preserves its exit code. Branch builds run one cluster job without a hosted
comparison or matrix. The earlier comparison reader remains available for
historical aggregate reports; the dispatch helpers accept only the cluster lane.

The hosted Nix Bazel Dev job and the duplicate project-pinned native Bazel job
are replaced by this lane for branch builds. Bazel 8.8.2 compatibility runs daily
at 07:00 UTC and on manual dispatch of `CI / Bazel`, using its separate lockfile.
It skips pull requests and branch pushes. Redis topology checks continue to run
on every pull request to `main`. Sanitizer,
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

The Bazel workflow file remains `parallel-cluster-ci.yaml`. Federation permits
that path and `nix.yaml`, each at its matching source branch. Their environment remains
`cluster-ci-benchmark`. Allow repository **branches** with the `**/*` deployment
branch pattern, not tag policies. The identity provider must require the exact
repository and owner IDs, an allowed workflow path at the signed branch ref, a
`refs/heads/` ref, a push/manual event and this environment subject.

Only users or integrations allowed to push to the source repository can start
this cluster lane. External fork PRs do not receive its environment secrets or
retained client. After reviewing an external contribution, a maintainer can
promote its exact commit to a repository branch such as `ci-approved/pr-123`;
that push runs the same cluster CI against the reviewed commit. Do not promote
unreviewed changes. The cluster job only accepts push/manual events; pull request
events select the separate hosted job.

## Before merging a pull request

`CI / Trusted build status` publishes `ci/trusted-builds` on the current PR
head. For changes outside `web/` and `.github/styles/`, success requires both
`CI / NativeLink` and `CI / Nix` to pass on that exact commit in the upstream
repository's push/manual lane. The retained-client jobs must actually pass;
a skipped job, a green local-only PR run, or results for an earlier commit do
not qualify. Nix's downstream installation and toolchain checks must pass too.
Newer failed or pending runs supersede earlier successes.

Repository branch pushes already start these builds. For a fork PR, a
maintainer must review the code first, including workflows and build scripts,
then promote the reviewed commit. From a trusted checkout, using your own
authenticated `gh` session, replace the values below with the PR number and
the **full commit SHA you reviewed**:

```sh
pr=123
reviewed_sha=FULL_REVIEWED_COMMIT_SHA
current_sha="$(gh api "repos/TraceMachina/nativelink/pulls/$pr" --jq .head.sha)"
test "$reviewed_sha" = "$current_sha" || { echo "PR changed; review its current commit"; exit 1; }
gh api --method POST repos/TraceMachina/nativelink/git/refs \
  -f "ref=refs/heads/ci-approved/pr-$pr-$reviewed_sha" \
  -f "sha=$reviewed_sha"
```

Creating this branch authorizes the reviewed code to run with the existing
trusted CI credentials and retained client. The command creates a new branch;
it never force-updates one. If the branch already exists, inspect or re-run its
builds instead. Do not use a workflow's `GITHUB_TOKEN` for promotion: its pushes
do not trigger the branch workflows. Ordinary fork workflow approval alone
does not authorize cluster access.

The status refreshes when branch builds start or finish and when a PR changes.
A new PR commit needs new successful builds. Missing builds remain pending,
and API errors clear any previous success before the reporter exits. Web/style
only changes receive an explicit exemption matching the build workflows' path
filters; renames out of source directories and incomplete file lists do not.

**Rollout:** after this workflow lands and reports its first status, add
`ci/trusted-builds` to the required checks for `main`, keeping the existing
requirements and the up-to-date branch policy. Select GitHub Actions as its
expected source. Require the published commit status, not the reporter job:
that job only reads metadata and publishes the result. Repository rules are
configured separately from workflow files; merging this change alone does not
make the status mandatory. See GitHub's
[required status check guidance](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks).

The reporter runs only trusted base/default-branch code with read access to
Actions and PR metadata and write access to commit statuses. It never checks
out fork code, executes artifacts, promotes a commit automatically, or receives
cluster credentials.

## Fork workflow approval

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

| Variable                          | Meaning                                            |
| --------------------------------- | -------------------------------------------------- |
| `NATIVELINK_CLUSTER_ENDPOINT`     | TLS REAPI endpoint, `grpcs://host:443`             |
| `NATIVELINK_CLUSTER_BEP_ENDPOINT` | TLS BEP endpoint                                   |
| `NATIVELINK_CLUSTER_INSTANCE`     | Remote instance name                               |
| `NATIVELINK_CLUSTER_RESULTS_URL`  | HTTPS build results URL prefix                     |
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
a file lock serializes writers and a bounded timeout includes lock waiting.
GitHub concurrency is per branch so a different PR cannot cancel another
branch's pending job. Dispatches wait for the retained checkout instead of
failing immediately when another build holds its lock.

The timeout test distinguishes a terminated zombie from a live child. That
expectation issue was discovered during a soak test in our systems; host reaping
timing must not decide whether the worker terminated the process.

## Shared Nix package

The Nix workflow builds the NativeLink package, installation check, toolchain
test wrapper and command list in one retained-client job. Nix shares their
common package dependency. The job exports their complete store closure as a
workflow artifact; the installation and ten toolchain jobs import it before
running their existing `nix run` checks. Each test still starts the NativeLink
binary built from the selected commit locally.

The manifest binds the artifact to an exact source revision, Linux platform,
output paths and checksum. Consumers evaluate their own output paths and
reject a mismatched, missing or damaged artifact before running tests. An
import failure stops the job rather than rebuilding the package on a hosted
runner. Re-run the producer when its three-day artifact retention expires.

Nix dispatch forwards no NativeLink API key, cache upload token or GitHub
credential. The hosted consumers use public read-only substitution, without
the cache upload watcher. Nightly Cargo and integration coverage keeps its
existing setup. Like Bazel, the shared build runs on reviewed repository
branch pushes and manual runs; reviewed external commits need promotion to a
repository branch. The identity provider must permit both workflow paths.

The transfer follows Nix's documented
[store export/import protocol](https://nix.dev/manual/nix/2.34/command-ref/nix-store/import).
An artifact from another workflow or an unreviewed source must not be imported
into a trusted build store.

## Reading results

The Actions summary reports source, setup plus workload time, each iteration's
build/test duration, exit status and cache/test counts. The JSON artifact contains
only constructed aggregate fields. It excludes raw events, credentials and
arbitrary server messages. Build and test time is combined because `bazel test`
builds its prerequisites in the same invocation. Setup includes waiting for
the retained client lock; GitHub scheduling before the job starts is excluded.

Local action, disk and remote hits are reconciled as:

`(local action hits + disk hits + remote hits) / (local action hits + process outcomes)`

Process outcomes exclude `total`, `internal` and duplicated `action cache hit`
runner counters. Missing local coverage yields an unknown total. Cached test
results overlap action reuse and remain separate. These definitions follow
[Bazel's SpawnStats](https://github.com/bazelbuild/bazel/blob/9.1.1/src/main/java/com/google/devtools/build/lib/runtime/SpawnStats.java).
