# Kubernetes example

This deployment sets up a 3-container deployment with separate CAS, scheduler
and worker.

Apply the manifests via kustomize:

```
kubectl apply -k .
```

Export the ports to the CAS and scheduler:

```
kubectl port-forward svc/native-link-service 50051:50051 50052:50052
```

In a separate terminal, run an example build:

```
bazel test //... \
  --remote_instance_name=main \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50052 \
  --remote_default_exec_properties=cpu_count=1
```
