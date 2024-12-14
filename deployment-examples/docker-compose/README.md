# NativeLink's Docker Compose deployment

This directory is a reference/starting point on creating a full Docker Compose
deployment of NativeLink's cache and remote execution system.

## Docker setup

1. Install [Docker](https://docs.docker.com/engine/install/) and
   [Docker Compose](https://docs.docker.com/compose/install/) on your system.
2. Open a terminal and run `docker compose up -d` in this directory to start the
   services.

It will take some time to apply, when it's finished everything should be
running. The endpoints are:

```sh
CAS/AC: 0.0.0.0:50051 # Configures CAS & AC for SSL connections
CAS/AC: 0.0.0.0:50071 # Configures CAS & AC for TLS connections
Scheduler: 0.0.0.0:50052
```

As a reference you should be able to compile this project using Bazel with
something like:

```sh
bazel test //... \
  --extra_toolchains=@rust_toolchains//:all \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50052 \
  --remote_default_exec_properties=cpu_count=1
```

> [!NOTE]
> The `nativelink` repository doesn't register any toolchains by default. The
> remote execution container in this example is currently a classic non-LRE
> container, so you'll have to add `--extra_toolchains=@rust_toolchains//:all`
> to the invocation to register `rules_rust`'s default toolchains. If you run
> this example against another `rules_rust` project you'll likely have these
> toolchains already registered in your `.bazelrc` or `MODULE.bazel` and can
> omit the flag.
>
> See: https://www.nativelink.com/docs/contribute/bazel

## Instances

All instances use the same Docker image, `trace_machina/nativelink:latest`,
built from the `Dockerfile` located at `./deployment-examples/docker-compose/Dockerfile`.

### CAS

The CAS (Content Addressable Storage) service is used as a local cache for the
NativeLink system. It's configured in the `docker-compose.yml` file under the
`nativelink_local_cas` service.

```yml
  nativelink_local_cas:
    image: trace_machina/nativelink:latest
    build:
      context: ../..
      dockerfile: ./deployment-examples/docker-compose/Dockerfile
      network: host
      args:
        - ADDITIONAL_SETUP_WORKER_CMD=${ADDITIONAL_SETUP_WORKER_CMD:-}
    volumes:
      - ${NATIVELINK_DIR:-~/.cache/nativelink}:/root/.cache/nativelink
      - type: bind
        source: .
        target: /root
    environment:
      RUST_LOG: ${RUST_LOG:-warn}
    ports:
      [
        "50051:50051/tcp",
        "127.0.0.1:50061:50061",
        "50071":50071/tcp",
      ]
    command: |
      nativelink /root/local-storage-cas.json
```

### Scheduler

The scheduler is currently the only single point of failure in the system. We
currently only support one scheduler at a time. The scheduler service is
responsible for scheduling tasks in the NativeLink system. It's configured in
the `docker-compose.yml` file under the nativelink_scheduler service.

```yml
  nativelink_scheduler:
    image: trace_machina/nativelink:latest
    build:
      context: ../..
      dockerfile: ./deployment-examples/docker-compose/Dockerfile
      network: host
      args:
        - ADDITIONAL_SETUP_WORKER_CMD=${ADDITIONAL_SETUP_WORKER_CMD:-}
    volumes:
      - type: bind
        source: .
        target: /root
    environment:
      RUST_LOG: ${RUST_LOG:-warn}
      CAS_ENDPOINT: nativelink_local_cas
    ports: [ "50052:50052/tcp" ]
    command: |
      nativelink /root/scheduler.json
```

### Workers

Worker instances are responsible for executing tasks. They're configured in the
`docker-compose.yml` file under the nativelink_executor service.

```yml
  nativelink_executor:
    image: trace_machina/nativelink:latest
    build:
      context: ../..
      dockerfile: ./deployment-examples/docker-compose/Dockerfile
      network: host
      args:
        - ADDITIONAL_SETUP_WORKER_CMD=${ADDITIONAL_SETUP_WORKER_CMD:-}
    volumes:
      - ${NATIVELINK_DIR:-~/.cache/nativelink}:/root/.cache/nativelink
      - type: bind
        source: .
        target: /root
    environment:
      RUST_LOG: ${RUST_LOG:-warn}
      CAS_ENDPOINT: nativelink_local_cas
      SCHEDULER_ENDPOINT: nativelink_scheduler
    command: |
      nativelink /root/worker.json
```

## Security

The Docker Compose setup is configured to run all services locally and isn't
suitable for production environments. The scheduler is a single point of failure
in the system.

The Docker Compose setup doesn't automatically delete old data. This could lead
to storage issues over time if not managed properly.

If you decide to stop using this setup, you can use `docker compose down` to
stop and remove all the containers. However, in a non-local setup, additional
steps may be required to ensure that all data is securely deleted.

## Future work

The Docker Compose setup doesn't automatically scale services. This could be
implemented using Docker Swarm or Kubernetes.

The Docker Compose setup doesn't automatically delete old data. This could be
implemented with a cleanup service or script.

## Useful tips

The Docker Compose setup can be modified for testing and development. For
example, you can change the RUST_LOG environment variable to control the logging
level of the services.

<img referrerpolicy="no-referrer-when-downgrade" src="https://nativelink.matomo.cloud/matomo.php?idsite=2&amp;rec=1&amp;action_name=deployment-examples%20docker-compose%20Readme.md" style="border:0" alt="" />
