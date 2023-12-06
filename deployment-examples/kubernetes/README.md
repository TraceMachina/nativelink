# Kubernetes example

This deployment sets up a 3-container deployment with separate CAS, scheduler
and worker. Don't use this example deployment in production. It's insecure.

In this example we're using `kind` to set up the cluster and `cilium` with
`metallb` to provide a `LoadBalancer` and `GatewayController`.

First set up a local development cluster:

```
./00_infra.sh
```

Next start a few standard deployments. This part also builds the remote
execution containers and makes them available to the cluster:

```
./01_operations.sh
```

Finally deploy NativeLink:

```
kubectl apply -k .
```

Now you can use the `k8s` configuration for Bazel to use the exposed remote
cache and executor:

```
bazel test --config=k8s //:dummy_test
```

When you're done testing, delete the cluster:

```
kind delete cluster
```
