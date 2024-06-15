#!/usr/bin/env bash

# Prepare the Kustomization and apply it to the cluster.

KUSTOMIZE_DIR=$(git rev-parse --show-toplevel)/deployment-examples/chromium

cat <<EOF > "$KUSTOMIZE_DIR"/kustomization.yaml
---
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
bases:
  - ../kubernetes/base

resources:
  - worker-chromium.yaml
EOF

cd "$KUSTOMIZE_DIR" && kustomize edit set image \
    nativelink=localhost:5001/nativelink:"$(\
        nix eval .#image.imageTag --raw)" \
    nativelink-worker-init=localhost:5001/nativelink-worker-init:"$(\
        nix eval .#nativelink-worker-init.imageTag --raw)" \
    nativelink-worker-chromium=localhost:5001/nativelink-worker-siso-chromium:"$(\
        nix eval .#nativelink-worker-siso-chromium.imageTag --raw)"

kubectl apply -k "$KUSTOMIZE_DIR"

kubectl rollout status deploy/nativelink-cas
kubectl rollout status deploy/nativelink-scheduler
kubectl rollout status deploy/nativelink-worker-chromium
