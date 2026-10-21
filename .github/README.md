# GitHub Actions CI structure

NativeLink CI uses two layers of reuse:

- Composite actions under `actions/` own repeatable setup, teardown, and
  publication steps.
- Workflows prefixed with `_` under `workflows/` are reusable workflow
  building blocks and are called with `workflow_call`.

Use a composite action when a caller still owns the job shape and needs to
insert project-specific steps. Use a reusable workflow when checkout, runner
selection, timeout, permissions, and the complete job lifecycle are common.

The main shared building blocks are:

- `run-nix-command`: prepare Nix, run one command, and always tear Nix down.
- `setup-bazel-ci`: standard disk cleanup, NativeLink Cloud configuration, and
  Bazel caches.
- `publish-image`: common Nix/image test and publication lifecycle.
- `generate-docs-pr`: generate docs and create or update the maintenance PR.
- `_nix-command.yaml`: reusable workflow for a complete Nix command job.

Keep trigger policy, matrices, job display names, and unusual permissions in
the caller workflow. Keep credentials and publication conditions explicit at
the call site unless they are identical for every caller.
