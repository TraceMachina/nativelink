# Kubernetes example

This deployment sets up a 3-container deployment with separate CAS, scheduler
and worker. Don't use this example deployment in production. It's insecure.

In this example we're using `kind` to set up the cluster `cilium` to provide a
`LoadBalancer` and `GatewayController`.

First set up a local development cluster:

```bash
./00_infra.sh
```

Next start a few standard deployments. This part also builds the remote
execution containers and makes them available to the cluster:

```bash
./01_operations.sh
```

Finally deploy NativeLink:

```bash
./02_application.sh
```

> [!TIP]
> You can use `./03_delete_application.sh` to remove just the nativelink
> deployments but leave the rest of the cluster intact.

This demo setup creates two gateways to expose the `cas` and `scheduler`
deployments via your local docker network:

```bash
CACHE=$(kubectl get gtw cache -o=jsonpath='{.status.addresses[0].value}')
SCHEDULER=$(kubectl get gtw scheduler -o=jsonpath='{.status.addresses[0].value}')

echo "Cache IP: $CACHE"
echo "Scheduler IP: $SCHEDULER"

# Prints something like:
#
# Cache IP: 172.20.255.4
# Scheduler IP: 172.20.255.5
```

You can now pass these IPs to your bazel invocation to use the remote cache and
executor:

```bash
bazel test \
    --config=lre \
    --remote_instance_name=main \
    --remote_cache=grpc://$CACHE:50051
    --remote_executore=grpc://$SCHEDULER:50052
    //:dummy_test
```

> [!TIP]
> You can add these flags to a to a `.bazelrc.user` file in the workspace root.
> Note that you'll need to pass in explicit IPs as this file can't resolve
> environment variables:
> ```bash
> # .bazelrc.user
> build --config=lre
> build --remote_instance_name=main
> build --remote_cache=grpc://172.20.255.4:50051
> build --remote_executor=grpc://172.20.255.5:50052
> ```

When you're done testing, delete the cluster:

```bash
kind delete cluster
```
