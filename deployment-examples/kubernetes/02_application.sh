# Get the nix derivation hash from the toolchain container, change the
# `TOOLCHAIN_TAG` variable in the `worker.json.template` to that hash and apply
# the configuration.

KUSTOMIZE_DIR=$(git rev-parse --show-toplevel)/deployment-examples/kubernetes

cat <<EOF > "$KUSTOMIZE_DIR"/kustomization.yaml
---
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
bases:
  - base

resources:
  - worker-lre-cc.yaml
  # TODO(aaronmondal): Fix java and add this:
  # - worker-lre-java.yaml
EOF

cd "$KUSTOMIZE_DIR" && kustomize edit set image \
    nativelink=localhost:5001/nativelink:$(\
        nix eval .#image.imageTag --raw) \
    nativelink-worker-lre-cc=localhost:5001/nativelink-worker-lre-cc:$(\
        nix eval .#nativelink-worker-lre-cc.imageTag --raw) \

# TODO(aaronmondal): Fix java and add this:
#   nativelink-worker-lre-java=localhost:5001/nativelink-worker-lre-java:$(\
#       nix eval .#nativelink-worker-lre-java.imageTag --raw)

kubectl apply -k "$KUSTOMIZE_DIR"

kubectl rollout status deploy/nativelink-cas
kubectl rollout status deploy/nativelink-scheduler
kubectl rollout status deploy/nativelink-worker-lre-cc

# TODO(aaronmondal): Fix java and add this:
# kubectl rollout status deploy/nativelink-worker-lre-java
