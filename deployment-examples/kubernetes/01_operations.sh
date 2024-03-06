# This script configures a cluster with a few standard deployments.

# TODO(aaronmondal): Add Grafana, OpenTelemetry and the various other standard
#                    deployments one would expect in a cluster.

set -xeuo pipefail

SRC_ROOT=$(git rev-parse --show-toplevel)

kubectl apply -f ${SRC_ROOT}/deployment-examples/kubernetes/gateway.yaml

# The image for the scheduler and CAS.
nix run .#image.copyTo \
    docker://localhost:5001/nativelink:local \
    -- \
    --dest-tls-verify=false

# The worker image for C++ actions.
nix run .#nativelink-worker-lre-cc.copyTo \
    docker://localhost:5001/nativelink-worker-lre-cc:local \
    -- \
    --dest-tls-verify=false

# The worker image for Java actions.
nix run .#nativelink-worker-lre-java.copyTo \
    docker://localhost:5001/nativelink-worker-lre-java:local \
    -- \
    --dest-tls-verify=false
