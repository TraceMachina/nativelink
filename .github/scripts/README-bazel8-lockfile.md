# Bazel 8 compatibility lockfile

The Bazel 8.8.2 lane in `native-bazel.yaml` copies `MODULE.bazel8.lock` to
`MODULE.bazel.lock` in its disposable checkout and runs with
`--lockfile_mode=error`. Other jobs keep the default lockfile for the version in
`.bazelversion`. Remote cache and execution settings are independent of this
selection.

Generate each lockfile with the Bazel version that consumes it. Even when two
versions accept the same lockfile schema, their module graphs and extension
results can differ. Do not convert a lockfile by editing `lockFileVersion`.

When dependencies or module extensions change, regenerate both lockfiles.
For the compatibility file, use Linux x86-64 to match the CI lane. With
Bazelisk available as `bazel`, run this from the repository root in a checkout
that has no other Bazel command running:

```bash
(
    set -eu
    backup=$(mktemp)
    cp MODULE.bazel.lock "$backup"
    trap 'cp "$backup" MODULE.bazel.lock; rm -f "$backup"' EXIT
    cp MODULE.bazel8.lock MODULE.bazel.lock
    USE_BAZEL_VERSION=8.8.2 bazel mod deps --lockfile_mode=update
    cp MODULE.bazel.lock MODULE.bazel8.lock
)
```

The command restores the default lockfile even if generation fails. Commit
`MODULE.bazel8.lock` with the dependency change. When updating the compatibility
lane's Bazel version, update the command above and regenerate this file too.

To check the lockfile without compiling, run the following in a disposable
checkout with Bazelisk:

```bash
cp MODULE.bazel8.lock MODULE.bazel.lock
USE_BAZEL_VERSION=8.8.2 bazel test //... --nobuild \
    --lockfile_mode=error --extra_toolchains=@rust_toolchains//:all
```

This checks resolution and analysis. The CI lane runs the full test suite;
analysis alone does not establish that compilation or tests pass.
