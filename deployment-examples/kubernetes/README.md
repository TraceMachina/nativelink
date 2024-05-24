# Kubernetes example

This deployment sets up a 4-container deployment with separate CAS, scheduler
and worker. Don't use this example deployment in production. It's insecure.

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
> You can use `./03_delete_application.sh` to remove just the `nativelink`
> deployments but leave the rest of the cluster intact.

This demo setup creates two gateways to expose the `cas` and `scheduler`
deployments via your local docker network:

```bash
CACHE=$(kubectl get gtw cache-gateway -o=jsonpath='{.status.addresses[0].value}')
SCHEDULER=$(kubectl get gtw scheduler-gateway -o=jsonpath='{.status.addresses[0].value}')

echo "Cache IP: $CACHE"
echo "Scheduler IP: $SCHEDULER"

# Prints something like:
#
# Cache IP: 172.20.255.4
# Scheduler IP: 172.20.255.5
```

You can now pass these IP addresses to your Bazel invocation to use the remote
cache and executor:

```bash
bazel build \
    --config=lre \
    --remote_instance_name=main \
    --remote_cache=grpc://$CACHE \
    --remote_executor=grpc://$SCHEDULER \
    //local-remote-execution/examples:hello_lre
```

> [!TIP]
> You can add these flags to a to a `.bazelrc.user` file in the workspace root.
> Note that you'll need to pass in explicit IP addresses as this file can't
> resolve environment variables:
> ```bash
> # .bazelrc.user
> build --config=lre
> build --remote_instance_name=main
> build --remote_cache=grpc://172.20.255.4
> build --remote_executor=grpc://172.20.255.5
> ```

When you're done testing, delete the cluster:

```bash
kind delete cluster
```

## Use a published image

[Published images](https://github.com/TraceMachina/nativelink/pkgs/container/nativelink) can be found under the Container registry, which uses the namespace `https://ghcr.io`. When using the Container registry, you can select prebuilt images and avoid building the image yourself.

To pull an existing image, you can run:

```sh
docker pull ghcr.io/tracemachina/nativelink:taggedImageVersion
```

## Derive a Tag for an OCI image

To derive the tag of the NativeLink image at a specific commit, run the below command and change `someCommit` with the commit hash you want to use:

```sh
nix eval github:TraceMachina/nativelink/someCommit#image.imageTag --raw
```

Alternatively, the tag can be derived from the upstream sources at the current state of the upstream main branch by running this command:

```sh
nix eval github:TraceMachina/nativelink#image.imageTag --raw
```

Similarly, you can also clone or checkout a specific version or commit of the NativeLink git repository to evaluate the output of the entire NativeLink flake. For example, assuming you've done the [NativeLink Getting Started Guide](https://github.com/TraceMachina/nativelink?tab=readme-ov-file#getting-started-with-nativelink) and cloned the repository, you can run these sample commands:

```sh
git log
git checkout commitHash
nix eval .#image.imageTag --raw
```
The `--raw` removes the surrounding quotes from the output string.

> [!WARNING]
> We don't recommend using this command to
> retrieve an image:
> ```sh
> nix eval github:TraceMachina/nativelink#image.imageTag --raw
> ```
> Using this command prevents anyone from
> identifying the specific version of the
> NativeLink container in use because
> reflects the image version available at the
> time of download. It'll be hard to debug,
> revert to previous versions if there are issues
> and complicate bug tracking.
> It's for these same reasons you won't be able
> to retrieve an image using the `latest` tag.

## Build and copy an OCI image

You can build and copy the image to a container registry with the `copyTo` attribute. Below are examples within the NativeLink repository for building and copying an image:
- [Example 1](https://github.com/TraceMachina/nativelink/blob/09b32c94d3cc7780816585e9b87f69c56cf931ae/deployment-examples/kubernetes/01_operations.sh#L12-L16) highlights:

```sh
nix run github:tracemachina/nativelink#image.copyTo <your destination>
```

- [Example 2](https://github.com/TraceMachina/nativelink/blob/09b32c94d3cc7780816585e9b87f69c56cf931ae/tools/local-image-test.nix#L12-L13) highlights how to skip pushing to an intermediary registry by copying directly to the docker-daemon:

```sh
IMAGE_NAME=$(nix eval .#image.imageName --raw)
IMAGE_TAG=$(nix eval .#image.imageTag --raw)

nix run .#image.copyTo docker-daemon:"${IMAGE_NAME}":"${IMAGE_TAG}"
```

You can find more about details around [nix](https://github.com/nlewo/nix2container). Published images are signed using `cosign`. For more details of the verification process of publishing OCI images see [SECURITY.md](https://github.com/TraceMachina/nativelink/blob/main/SECURITY.md)

## NativeLink Community

Reach out to the [NativeLink Slack community](https://join.slack.com/t/nativelink/shared_invite/zt-2forhp5n9-L7dTD21nCSY9_IRteQvZmw) for any questions via #NativeLink!
