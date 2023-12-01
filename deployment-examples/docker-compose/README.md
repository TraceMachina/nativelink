# Native Link's Docker Compose Deployment

This directory contains a reference/starting point on creating a full Docker Compose deployment of Native Link's cache and remote execution system.

## Docker Setup

1. Ensure [Docker](https://docs.docker.com/engine/install/) and [Docker Compose](https://docs.docker.com/compose/install/) are installed on your system.
2. Open a terminal and run `docker-compose up -d` in this directory to start the services.

It will take some time to apply, when it is finished everything should be running. The endpoints are:

```sh
CAS/AC: 0.0.0.0:50051 # Configures CAS & AC for SSL connections
CAS/AC: 0.0.0.0:50071 # Configures CAS & AC for TLS connections
Scheduler: 0.0.0.0:50052
```

As a reference you should be able to compile this project using bazel with something like:

```sh
bazel test //... \
  --remote_instance_name=main \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50052 \
  --remote_default_exec_properties=cpu_count=1
```

## Instances

All instances use the same Docker image, trace_machina/native-link:latest, built from the Dockerfile located at ./deployment-examples/docker-compose/Dockerfile.

### CAS

The CAS (Content Addressable Storage) service is used as a local cache for the Native Link system. It is configured in the docker-compose.yml file under the native_link_local_cas service.

```yml
  native_link_local_cas:
    image: trace_machina/native-link:latest
    build:
      context: ../..
      dockerfile: ./deployment-examples/docker-compose/Dockerfile
      network: host
      args:
        - ADDITIONAL_SETUP_WORKER_CMD=${ADDITIONAL_SETUP_WORKER_CMD:-}
    volumes:
      - ${NATIVE_LINK_DIR:-~/.cache/native-link}:/root/.cache/native-link
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
      native-link /root/local-storage-cas.json
```

### Scheduler

The scheduler is currently the only single point of failure in the system. We currently only support one scheduler at a time.
The scheduler service is responsible for scheduling tasks in the Native Link system. It is configured in the docker-compose.yml file under the native_link_scheduler service.

```yml
  native_link_scheduler:
    image: trace_machina/native-link:latest
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
      CAS_ENDPOINT: native_link_local_cas
    ports: [ "50052:50052/tcp" ]
    command: |
      native-link /root/scheduler.json
```

### Workers

Worker instances are responsible for executing tasks. They are configured in the docker-compose.yml file under the native_link_executor service.

```yml
  native_link_executor:
    image: trace_machina/native-link:latest
    build:
      context: ../..
      dockerfile: ./deployment-examples/docker-compose/Dockerfile
      network: host
      args:
        - ADDITIONAL_SETUP_WORKER_CMD=${ADDITIONAL_SETUP_WORKER_CMD:-}
    volumes:
      - ${NATIVE_LINK_DIR:-~/.cache/native-link}:/root/.cache/native-link
      - type: bind
        source: .
        target: /root
    environment:
      RUST_LOG: ${RUST_LOG:-warn}
      CAS_ENDPOINT: native_link_local_cas
      SCHEDULER_ENDPOINT: native_link_scheduler
    command: |
      native-link /root/worker.json
```

## Security

The Docker Compose setup is configured to run all services locally, which may not be suitable for production environments. The scheduler is currently the only single point of failure in the system. If the scheduler service fails, the entire system will be affected.

The Docker Compose setup does not automatically delete old data. This could lead to storage issues over time if not managed properly.

If you decide to stop using this setup, you can use `docker-compose down` to stop and remove all the containers. However, in a non-local setup, additional steps may be required to ensure that all data is securely deleted.

## Future work / TODOs

Currently, the Docker Compose setup does not automatically scale services. This could be implemented using Docker Swarm or Kubernetes.
The Docker Compose setup does not automatically delete old data. This could be implemented with a cleanup service or script.

## Useful tips

The Docker Compose setup can be easily modified for testing and development. For example, you can change the RUST_LOG environment variable to control the logging level of the services.
