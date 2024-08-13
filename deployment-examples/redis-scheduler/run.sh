#!/usr/bin/env bash

### START DOCKER ###
# Start the redis container if it doesn't exist
IMAGE_NAME="redis/redis-stack:latest"
CONTAINER_NAME="redis-stack"

[[ $(docker ps -al -f name=$CONTAINER_NAME --format '{{.Names}}') == $CONTAINER_NAME ]] ||
docker run -d --name $CONTAINER_NAME $IMAGE_NAME
### END DOCKER ###

### Gets the absolute path to the dir this script is running in ###
WORKING_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

export CA=$WORKING_DIR/example-do-not-use-in-prod-rootca.crt
export PEM=$WORKING_DIR/example-do-not-use-in-prod-key.pem
export NATIVELINK_CACHE_DIR=$WORKING_DIR/.cache/nativelink
export SCHEDULER_BACKEND_1_ENDPOINT=127.0.0.1:51061
export SCHEDULER_BACKEND_2_ENDPOINT=127.0.0.1:52061
export SCHEDULER_FRONTEND_ENDPOINT=127.0.0.1:50061

export WORKER_1_FAST_CAS=$NATIVELINK_CACHE_DIR/worker_1_data/cas
export WORKER_2_FAST_CAS=$NATIVELINK_CACHE_DIR/worker_2_data/cas

export CAS_PATH=$NATIVELINK_CACHE_DIR/cas
export AC_PATH=$NATIVELINK_CACHE_DIR/ac
export RUST_LOG=debug,h2=off,tonic=info,tower=off,hyper=off
export WORKER_1_WORK_DIR=$NATIVELINK_CACHE_DIR/worker_1
export WORKER_2_WORK_DIR=$NATIVELINK_CACHE_DIR/worker_2

# Clean up everything. You will have to restart all services after running this
if [ $1 = "clean" ]; then
  if [ $2 = "redis" ]; then
    docker exec -it redis-stack redis-cli flushdb
  elif [ $2 = "all" ]; then
    rm -rf $CAS_PATH $AC_PATH $WORKER_2_WORK_DIR $WORKER_1_WORK_DIR $WORKER_1_FAST_CAS $WORKER_2_FAST_CAS /tmp/nativelink
    docker exec -it redis-stack redis-cli flushdb
  fi
else
  if [ $1 = "cas" ]; then
    CONFIG_FILE=$WORKING_DIR/local-storage-cas.json
  elif [ $1 = "frontend" ]; then
    CONFIG_FILE=$WORKING_DIR/scheduler_frontend.json
  elif [ $1 = "backend_1" ]; then
    CONFIG_FILE=$WORKING_DIR/scheduler_backend_1.json
  elif [ $1 = "backend_2" ]; then
    CONFIG_FILE=$WORKING_DIR/scheduler_backend_2.json
  elif [ $1 = "worker_1" ]; then
    rm -rf $WORKER_1_FAST_CAS
    CONFIG_FILE=$WORKING_DIR/worker_1.json
  elif [ $1 = "worker_2" ]; then
    rm -rf $WORKER_2_FAST_CAS
    CONFIG_FILE=$WORKING_DIR/worker_2.json
  fi
  cargo run --bin nativelink -- $CONFIG_FILE
fi
