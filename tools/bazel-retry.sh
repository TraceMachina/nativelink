#!/bin/bash
set -o pipefail
BAZEL_LOG=$(mktemp -t)
# Bazel's downloader retry only covers truncated downloads, not HTTP 5xx/403 failures,
# so a flaky fetch aborts the build with no retry. We retry the download in this case.
delay=5
for attempt in 1 2 3; do
    if exec bazel "$@" 2>&1 | tee "${BAZEL_LOG}"; then
        exit 0
    fi
    grep -E '^ERROR:' "${BAZEL_LOG}" |
        grep -Eq 'GET returned (403|429|5[0-9][0-9])|Bad Gateway|Connection (reset|timed out)|read timed out|Could not resolve host' || {
        echo "Bazel failed (non-transient); not retrying"
        exit 1
    }
    [ "$attempt" -lt 3 ] || break
    echo "Transient fetch error (attempt ${attempt}); retrying in ${delay}s..."
    sleep "$delay"
    delay=$((delay * 2))
done
echo "Bazel still failing after 3 attempts."
exit 1
