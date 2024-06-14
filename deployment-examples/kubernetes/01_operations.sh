# This script configures a cluster with a few standard deployments.

# TODO(aaronmondal): Add Grafana, OpenTelemetry and the various other standard
#                    deployments one would expect in a cluster.

set -xeuo pipefail

SRC_ROOT=$(git rev-parse --show-toplevel)

curl -v \
    -H 'content-Type: application/json' \
    -d '{"flakeOutput": "./src_root#image"}' \
    localhost:8082/eventlistener

curl -v \
    -H 'content-Type: application/json' \
    -d '{"flakeOutput": "./src_root#nativelink-worker-lre-cc"}' \
    localhost:8082/eventlistener

until kubectl get pipelinerun \
        -l tekton.dev/pipeline=rebuild-nativelink | grep -q 'NAME'; do
    echo "Waiting for PipelineRuns to start..."
    sleep 0.1
done

printf 'Waiting for PipelineRuns to finish...

You may cancel this script now and use `tkn pr ls` and `tkn pr logs -f` to
monitor the PipelineRun logs.

'

kubectl wait \
    --for=condition=Succeeded \
    --timeout=55m \
    pipelinerun \
        -l tekton.dev/pipeline=rebuild-nativelink
