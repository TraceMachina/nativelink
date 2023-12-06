# Kubernetes example

This deployment sets up a 3-container deployment with separate CAS, scheduler
and worker. Don't use this example deployment in production. It's insecure.

1. Start the local cluster cluster

```
kind cluster create --config=cluster.yaml
```

2. Apply the manifests via kustomize:

```
kubectl apply -k .
```

3. Expose the ports the ports to the CAS and scheduler:

```
kubectl port-forward svc/native-link-cas 50051 &
kubectl port-forward svc/native-link-scheduler 50052 &
```

4. In a separate terminal, run an example build:

```
bazel test //... \
  --remote_instance_name=main \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50052 \
  --remote_default_exec_properties=cpu_count=1
```

5. When you're done testing, delete the cluster:

```
kind delete cluster
```
