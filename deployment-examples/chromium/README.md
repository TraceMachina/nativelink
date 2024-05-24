# Chromium example

This deployment sets up a 4-container deployment with separate CAS, scheduler
and worker. Don't use this example deployment in production. It's insecure.

> [!WARNING]
> - The client build request is best done from a Ubuntu image, `./03_build_chrome_tests.sh`. It will check if the image is Ubuntu and
> fail otherwise.
> - This tutorial has been tested in a Nix environment of version `2.
> 21.0`.
> - You need to install the [Docker](https://docs.docker.com/engine/install/ubuntu/) Engine in Ubuntu.
> - To get your Nix environment set up see the [official Nix installation documentation](https://nix.dev/install-nix).

All commands should be run from nix to ensure all dependencies exist in the environment.

```bash
nix develop
```

In this example we're using `kind` to set up the cluster `cilium` to provide a
`LoadBalancer` and `GatewayController`.

First set up a local development cluster:

```bash
native up
```

> [!TIP]
> The `native up` command uses Pulumi under the hood. You can view and delete
> the stack with `pulumi stack` and `pulumi destroy`.

Next start a few standard deployments. This part also builds the remote
execution containers and makes them available to the cluster:

```bash
./01_operations.sh
```

> [!TIP]
> The operations invoke cluster-internal Tekton Pipelines to build and push the
> `nativelink` and worker images. You can view the state of the pipelines with
> `tkn pr ls` and `tkn pr logs`/`tkn pr logs --follow`.

Finally, deploy NativeLink:

```bash
./02_application.sh
```

> [!TIP]
> You can use `./04_delete_application.sh` to remove just the `nativelink`
> deployments but leave the rest of the cluster intact.

This demo setup creates two gateways to expose the `cas` and `scheduler`
deployments via your local docker network:

```bash
CACHE=$(kubectl get gtw cache-gateway -o=jsonpath='{.status.addresses[0].value}')
SCHEDULER=$(kubectl get gtw scheduler-gateway -o=jsonpath='{.status.addresses[0].value}')

echo "Cache IP: $CACHE"
echo "Scheduler IP: $SCHEDULER"
```

Using `./03_build_chrome_tests.sh` example script will download needed dependencies
for building Chromium unit tests using NativeLink CAS and Scheduler. The initial part
of the script checks if some dependencies exist, if not installs them, then moves on
to downloading and building Chromium tests. The script simplifies the setup described
in [linux/build_instructions.md](https://chromium.googlesource.com/chromium/src/+/main/docs/linux/build_instructions.md)

```bash
./03_build_chrome_tests.sh
```

> [!TIP]
> You can monitor the logs of container groups with `kubectl logs`:
> ```bash
> kubectl logs -f -l app=nativelink-cas
> kubectl logs -f -l app=nativelink-scheduler
> kubectl logs -f -l app=nativelink-worker-chromium --all-containers=true
> watch $HOME/chromium/src/buildtools/reclient/reproxystatus
> ```

When you're done testing, delete the cluster:

```bash
kind delete cluster
```
## NativeLink Community
If you have any questions, please reach out to the [NativeLink Community](https://join.slack.com/t/nativelink/shared_invite/zt-2i2mipfr5-lZAEeWYEy4Eru94b3IOcdg).
