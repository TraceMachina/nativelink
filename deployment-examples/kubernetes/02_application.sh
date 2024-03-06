# Get the nix derivation hash from the toolchain container, change the
# `TOOLCHAIN_TAG` variable in the `worker.json.template` to that hash and apply
# the configuration.

KUSTOMIZE_DIR=$(git rev-parse --show-toplevel)/deployment-examples/kubernetes

sed "s/__LRE_CC_TOOLCHAIN_TAG__/$(nix eval .#lre-cc.imageTag --raw)/g" \
  "$KUSTOMIZE_DIR/worker-lre-cc.json.template" \
  > "$KUSTOMIZE_DIR/worker-lre-cc.json" \

sed "s/__LRE_JAVA_TOOLCHAIN_TAG__/$(nix eval .#lre-java.imageTag --raw)/g" \
  "$KUSTOMIZE_DIR/worker-lre-java.json.template" \
  > "$KUSTOMIZE_DIR/worker-lre-java.json" \

kubectl apply -k "$KUSTOMIZE_DIR"

kubectl rollout status deploy/nativelink-cas
kubectl rollout status deploy/nativelink-scheduler
kubectl rollout status deploy/nativelink-worker-lre-cc
kubectl rollout status deploy/nativelink-worker-lre-java
