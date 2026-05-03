#!/usr/bin/env bash
# Compiles and runs the warm-worker COW isolation Java demo.
#
# Demonstrates the difference between a shared warm worker (tenant secrets
# leak across jobs) and the COW-isolated warm worker introduced by this PR
# (tenant writes are confined to a per-job overlay).
#
# Exits non-zero if the isolation contract regresses.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
JAVA_DIR="${SCRIPT_DIR}/../docker/java/warmup"

if ! command -v javac > /dev/null 2>&1; then
    echo "javac not found - install a JDK (e.g. openjdk-21-jdk) and retry" >&2
    exit 1
fi

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "${WORK_DIR}"' EXIT

cp "${JAVA_DIR}/WarmWorkerIsolationTest.java" "${WORK_DIR}/"

(
    cd "${WORK_DIR}"
    javac WarmWorkerIsolationTest.java
    java WarmWorkerIsolationTest
)
