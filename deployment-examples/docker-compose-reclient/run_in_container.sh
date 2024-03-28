#!/bin/sh

# This script is run instead of the provided entrypoint in the action provided
# to the worker.  It is passed the full entrypoint.  This allows it to be
# executed in a container rather than on the host machine.  In this case, we are
# already running in a container, so we execute Docker-in-Docker which causes
# a few headaches about volumes to mount.

# If a container is specified and starts with docker:// then run the command in Docker
if [ "$(echo "$CONTAINER" | cut -c-9)" = "docker://" ]; then
    # Top level work directory is /root/.cache/nativelink/work/xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
    # However, we are doing Docker-in-Docker and therefore the work directory is
    # on the host and we need to account for that.  This strips off the
    # /root/.cache/nativelink and that is populated with $HOST_ROOT from the
    # worker.json which is then populated by ${NATIVE_LINK_DIR} from the
    # docker-compose.yml.
    WORK_DIRECTORY=$(pwd | cut -c24-93)
    WORKING_DIRECTORY=$(pwd | cut -c94-)
    CONTAINER_NAME=$(echo "$CONTAINER" | cut -c10-)
    docker pull -q $CONTAINER_NAME > /dev/null
    exec docker run --rm --network none -w /work${WORKING_DIRECTORY} -v ${HOST_ROOT}${WORK_DIRECTORY}:/work $CONTAINER_NAME /bin/env "$@"
fi

# Default to simply running the command outside of a container
exec "$@"
