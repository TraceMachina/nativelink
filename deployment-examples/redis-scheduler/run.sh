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

rm -rf $CAS_PATH $AC_PATH $WORKER_1_WORK_DIR $WORKER_1_FAST_CAS $WORKER_2_WORK_DIR  $WORKER_2_FAST_CAS /tmp/nativelink
docker exec -it redis-stack redis-cli flushdb

tmux new-session \; \
  new-window "RUST_LOG=debug,h2=off,tonic=info,tower=off,hyper=off,nativelink_service=off cargo run --bin nativelink -- $WORKING_DIR/scheduler_frontend.json" \; \
  split-window -h "cargo run --bin nativelink -- $WORKING_DIR/scheduler_backend_1.json" \; \
  split-window -v "cargo run --bin nativelink -- $WORKING_DIR/scheduler_backend_2.json" \; \
  attach
