#!/bin/sh

# If a container is specified and starts with docker:// then run the command in Docker
if [ "$(echo "$CONTAINER" | cut -c-9)" = "docker://" ]; then
    # Top level work directory is /root/.cache/turbo-cache/work/xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
    # However, we are doing Docker-in-Docker and therefore the work directory is
    # on the host and we need to account for that.  This strips off the
    # /root/.cache/turbo-cache and that is populated with $HOST_ROOT from the
    # worker.json which is then populated by ${TURBO_CACHE_DIR} from the
    # docker-compose.yml.
    WORK_DIRECTORY=$(pwd | cut -c25-94)
    WORKING_DIRECTORY=$(pwd | cut -c95-)
    exec docker run --rm -w /work${WORKING_DIRECTORY} -v ${HOST_ROOT}${WORK_DIRECTORY}:/work $(echo "$CONTAINER" | cut -c10-) /bin/env "$@"
fi

# Default to simply running the command outside of a container
exec "$@"
