# NativeLink cluster CI

**CI / NativeLink** runs `bazel test //...` on the retained cluster client for
branch pushes and manual runs. Repository PRs and listed forks reuse this
build instead of repeating the full suite on a hosted PR client. Other forks
run the same suite with the project-pinned Bazel on a hosted Linux runner,
using local execution and the public read-only cache. Both paths include Clippy
and rustfmt.
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

The cluster lanes run on the AWS deployment, `ci.nativelink.com`. The Bazel
workflow file remains `parallel-cluster-ci.yaml`. The dispatch role trusts that
path and `nix.yaml`, each at its matching source branch, in the
`cluster-ci-aws` environment. Allow repository **branches** with the `*` and
`**/*` deployment branch patterns, not tag policies. The role's trust requires
the repository, an allowed workflow path at a `refs/heads/` ref and this
environment subject. The role and the environment are set up from
`TraceMachina/ci-acceleration` (`infrastructure/terraform/aws/bootstrap`).

Only upstream branch runs can use the retained client. The trusted coordinator
can promote a listed fork's exact commit to an upstream branch and dispatch
the existing workflows. Unlisted forks need explicit maintainer review and
promotion. The cluster job still accepts only push/manual events; ordinary fork
PR jobs receive no cluster credentials.

## Before merging a pull request

`CI / Trusted build status` publishes `ci/trusted-builds` on the current PR
head. For changes outside `web/` and `.github/styles/`, success requires both
`CI / NativeLink` and `CI / Nix` to pass on that exact commit in the upstream
repository's push/manual lane. The retained-client jobs must actually pass;
a skipped job, a green local-only PR run, or results for an earlier commit do
not qualify. Nix's downstream installation and toolchain checks must pass too.
Newer failed or pending runs supersede earlier successes.

The trusted contributor list lives in [trusted-contributors.json](../trusted-contributors.json)
on the protected base branch. Each entry pins the contributor's GitHub account
ID and their fork's repository ID, with readable names. The PR author and fork
owner must match that entry, and the event sender must be a listed account.
An unlisted sender, transferred/recreated fork, or a PR from another repository
does not qualify. A fork cannot change its own trust by editing this file.
Listing an account authorizes its future qualifying commits to run with trusted
CI credentials and the retained client; it is not merely a cache permission.

On a qualifying PR update, the coordinator creates an immutable
`ci-approved/pr-NUMBER-FULL_SHA` branch and dispatches only missing workflows.
Existing successful, failed, or queued runs are reused; failures need an explicit
re-run. The hosted fork job reads routing policy from the base branch and skips
its Bazel suite for these contributions. Repository PRs already have push builds
and skip the hosted job entirely. If a path-filtered push missed the latest
commit of a PR that also changes source, the coordinator dispatches the missing
builds. Builds on merge to `main` remain unchanged.

Automatic Nix dispatch uses `scope=branch`, which builds the shared package and
runs installation/toolchain checks without starting nightly Cargo, BuildStream,
Buck2, or Mongo jobs. Manual dispatch defaults to `scope=all`; the nightly schedule
is unchanged. The coordinator uses `GITHUB_TOKEN` with explicit dispatch because
token-created branches do not generate push builds. See GitHub's
[workflow token behavior](https://docs.github.com/en/actions/concepts/security/github_token).

For an unlisted fork, review the code first, including workflows and build
scripts, then promote it. From a trusted checkout with your own authenticated
`gh` session, replace these values with the PR number and the **full commit SHA
you reviewed**:

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
builds instead. This manual command relies on a user token's push event; the
automatic coordinator instead dispatches explicitly. If GitHub rejects automatic
branch creation, for example because promoting changed workflow files needs
additional permissions, use this manual path. No broader token is installed by
the workflow. Ordinary fork workflow approval alone does not authorize cluster
access.

The status refreshes when builds finish and when a PR changes. Re-running a job
also refreshes the status when that attempt starts. First-attempt start events
do not allocate a reporter runner: the PR event already set a pending status.
A new PR commit needs new successful builds. Missing builds remain pending,
and API errors clear any previous success before the reporter exits. Web/style
only changes receive an explicit exemption matching the build workflows' path
filters; renames out of source directories and incomplete file lists do not.

**Rollout:** after this workflow lands and reports its first status, add
`ci/trusted-builds` to the required checks for `main`, keeping the existing
requirements and the up-to-date branch policy. Select GitHub Actions as its
expected source. Require the published commit status, not the reporter job:
that job coordinates builds and publishes the result. Repository rules are
configured separately from workflow files; merging this change alone does not
make the status mandatory. See GitHub's
[required status check guidance](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks).

The coordinator runs only trusted base/default-branch code. Its token can read
PR metadata, create repository branches, dispatch workflows, and write commit
statuses. It never checks out fork code, executes artifacts, or receives cluster
credentials. Forks must update from `main` to pick up the new routing and Nix
dispatch input. During rollout, a base that predates the policy falls back to
local fork tests.

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

Configure `NL_EKS_CLUSTER`, `NL_AWS_REGION`, `NL_GKE_NAMESPACE`,
`NL_GKE_BAZEL_POD` and `NL_GKE_NIX_POD` as environment variables, and
`NL_AWS_DISPATCH_ROLE_ARN`, `NL_GKE_SERVER` and `NL_GKE_CA_DATA` as environment
secrets. The cluster's address and CA are not credentials, but this repository
is public and a job log shows a variable's value; a secret is masked. The
variables keep their GKE names from the earlier deployment. GitHub OIDC gives
the job short-lived role credentials, and the AWS CLI signs a token for the one
cluster with them in memory.

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
