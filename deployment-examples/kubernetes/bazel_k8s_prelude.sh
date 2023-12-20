#!/usr/bin/env bash
# Copyright 2022 The Native Link Authors. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# --- begin runfiles.bash initialization v3 ---
# Copy-pasted from the Bazel Bash runfiles library v3.
set -uo pipefail; set +e; f=bazel_tools/tools/bash/runfiles/runfiles.bash
source "${RUNFILES_DIR:-/dev/null}/$f" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "${RUNFILES_MANIFEST_FILE:-/dev/null}" | cut -f2- -d' ')" 2>/dev/null || \
  source "$0.runfiles/$f" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "$0.runfiles_manifest" | cut -f2- -d' ')" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "$0.exe.runfiles_manifest" | cut -f2- -d' ')" 2>/dev/null || \
  { echo>&2 "ERROR: cannot find $f"; exit 1; }; f=; set -e
# --- end runfiles.bash initialization v3 ---

NATIVELINK_TAG=$(cat "$(rlocation nativelink-current-tag/bin/nativelink-current-tag)")
KUSTOMIZE_DIR=$(rlocation nativelink/deployment-examples/kubernetes)

remove_resources() {
    kubectl kustomize \
        --load-restrictor LoadRestrictionsNone \
        "$KUSTOMIZE_DIR" \
        | kubectl delete -f - \
        || echo "Resource cleanup failed. Manually verify your cluster." >&2
}

trap remove_resources EXIT

sed "s/__NATIVELINK_TOOLCHAIN_TAG__/${NATIVELINK_TAG}/g" \
  "$KUSTOMIZE_DIR/worker.json.template" \
  > "$KUSTOMIZE_DIR/worker.json"

kubectl kustomize \
    --load-restrictor LoadRestrictionsNone \
    "$KUSTOMIZE_DIR" \
    | kubectl apply -f -

kubectl rollout status deploy/nativelink-cas
kubectl rollout status deploy/nativelink-scheduler
kubectl rollout status deploy/nativelink-worker

# Application code will run here.
