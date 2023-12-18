#!/usr/bin/env bash
# Get the nix derivation hash from the toolchain container, change the
# `TOOLCHAIN_TAG` variable in the `worker.json.template` to that hash and apply
# the configuration.

KUSTOMIZE_DIR=$(git rev-parse --show-toplevel)/deployment-examples/kubernetes

sed "s/__NATIVELINK_TOOLCHAIN_TAG__/$(nix eval .#lre.imageTag --raw)/g" \
  "$KUSTOMIZE_DIR/worker.json.template" \
  > "$KUSTOMIZE_DIR/worker.json"

kubectl apply -k "$KUSTOMIZE_DIR"

kubectl rollout status deploy/nativelink-cas
kubectl rollout status deploy/nativelink-scheduler
kubectl rollout status deploy/nativelink-worker

# Verify endpoint reachability.
INSECURE_CACHE=$(kubectl get gtw insecure-cache -o=jsonpath='{.status.addresses[0].value}')
SCHEDULER=$(kubectl get gtw scheduler -o=jsonpath='{.status.addresses[0].value}')
CACHE=$(kubectl get gtw cache -o=jsonpath='{.status.addresses[0].value}')
PROMETHEUS=$(kubectl get gtw prometheus -o=jsonpath='{.status.addresses[0].value}')

printf "
Insecure Cache IP: $INSECURE_CACHE -> --remote_cache=grpc://$INSECURE_CACHE:50051
Cache IP: $CACHE
Scheduler IP: $SCHEDULER -> --remote_executor=grpc://$SCHEDULER:50052
Prometheus IP: $PROMETHEUS

Insecure cache status: $(curl http://"$INSECURE_CACHE":50051/status)
Cache status: $(curl https://"$CACHE":50071/status)
Scheduler status: $(curl http://"$SCHEDULER":50052/status)
Prometheus status: $(curl http://"$PROMETHEUS":50061/status)
"
