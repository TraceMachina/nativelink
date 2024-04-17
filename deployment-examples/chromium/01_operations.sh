# This script configures a cluster with a few standard deployments.

# TODO(aaronmondal): Add Grafana, OpenTelemetry and the various other standard
#                    deployments one would expect in a cluster.

set -xeuo pipefail

SRC_ROOT=$(git rev-parse --show-toplevel)

EVENTLISTENER=$(kubectl get gtw eventlistener -o=jsonpath='{.status.addresses[0].value}')

# The image for the scheduler and CAS.
curl -v \
    -H 'content-Type: application/json' \
    -d '{
        "flakeOutput": "./src_root#image",
        "imageTagOverride": "local"
    }' \
    http://${EVENTLISTENER}:8080

# Wrap it nativelink to turn it into a worker.
curl -v \
    -H 'content-Type: application/json' \
    -d '{
        "flakeOutput": "./src_root#nativelink-worker-siso-chromium",
        "imageTagOverride": "local"
    }' \
    http://${EVENTLISTENER}:8080

# Wait for the pipelines to finish.
kubectl wait \
    --for=condition=Succeeded \
    --timeout=30m \
    pipelinerun \
        -l tekton.dev/pipeline=rebuild-nativelink
