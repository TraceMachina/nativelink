#!/usr/bin/env bash
# bazel test //... on either Buildkite lane. The trusted lane runs actions
# on ci.nativelink.com and fills its cache from main. The untrusted lane
# builds in its own pod and only reads that cache.
set -euo pipefail

endpoint=grpcs://ci.nativelink.com:443
worker_image=docker.io/library/node:22-bookworm@sha256:0e5f906573693feaa1e21057ebdcfdb5bd5021f050b2dc7c9deceb629c7da2a8

flags=(
    --config=nl-toolchain
    --config=nl-quiet
    --lockfile_mode=error
    --verbose_failures
    --build_metadata=ROLE=CI
    --build_metadata=BUILDKITE_PIPELINE="$BUILDKITE_PIPELINE_SLUG"
    --build_metadata=BUILDKITE_BUILD_URL="$BUILDKITE_BUILD_URL"
    --build_metadata=COMMIT_SHA="$BUILDKITE_COMMIT"
    # Part of every action's cache key: both lanes must pass the same ones,
    # or untrusted lookups never match what the trusted lane wrote.
    --extra_toolchains=@@rules_python++python+pythons_hub//:all
    --remote_default_exec_properties=container-image="$worker_image"
    --remote_default_exec_properties=cpu_count=1000
    --remote_default_exec_properties=memory_kb=2000000
    --remote_default_exec_properties=disk_kb=1000000
)

# The credential helper adds the key to requests for the endpoint, so the
# key appears in no command line or rc file.
cache_flags=(
    --config=nl-cache
    --remote_cache="$endpoint"
    --remote_instance_name=main
    --credential_helper=ci.nativelink.com="$PWD/.github/scripts/benchmark_credentials.py"
    --remote_header=x-nativelink-project=nativelink-github-ci
)
NL_ENDPOINTS=$endpoint
export NL_ENDPOINTS

if [[ $BUILDKITE_PIPELINE_SLUG == *-forks ]]; then
    # Fork code: a key that can only read the cache, if one is set. Fork code
    # can read it too, which is why it must never be able to write.
    if NL_API_KEY=$(buildkite-agent secret get NATIVELINK_READ_KEY 2> /dev/null); then
        export NL_API_KEY
        flags+=("${cache_flags[@]}" --remote_upload_local_results=false)
    else
        echo "No read-only cache key in this cluster: building without a remote cache."
    fi
    exec ./tools/bazel-retry.sh test //... "${flags[@]}"
fi

# Only main fills the shared cache. Remote actions are cached either way.
upload=false
if [ "$BUILDKITE_BRANCH" = main ] && [ "$BUILDKITE_PULL_REQUEST" = false ]; then
    upload=true
fi

NL_API_KEY=$(buildkite-agent secret get NATIVELINK_API_KEY)
export NL_API_KEY

flags+=(
    "${cache_flags[@]}"
    --config=nl-rbe
    --remote_executor="$endpoint"
    --bes_backend="$endpoint"
    --bes_header=x-nativelink-project=nativelink-github-ci
    --bes_results_url=https://ci.nativelink.com/a/loadtest/build/
    --remote_upload_local_results="$upload"
    --jobs=48
)

# The same shell as the retained cluster client. Host Bazel uses its own
# runtime, not the shell's C++ library path.
exec nix develop --impure --command \
    env -u LD_LIBRARY_PATH ./tools/bazel-retry.sh test //... "${flags[@]}"
