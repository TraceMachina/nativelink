#!/usr/bin/env bash

### START DOCKER ###
# Start the redis container if it doesn't exist
IMAGE_NAME="redis/redis-stack:latest"
CONTAINER_NAME="redis-stack"

### END DOCKER ###

### Gets the absolute path to the dir this script is running in ###
WORKING_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

export NATIVELINK_CACHE_DIR=$WORKING_DIR/.cache/nativelink
export CAS_PATH=$NATIVELINK_CACHE_DIR/cas
export AC_PATH=$NATIVELINK_CACHE_DIR/ac

export SCHEDULER_BACKEND_1_ENDPOINT=127.0.0.1:51061
export SCHEDULER_BACKEND_2_ENDPOINT=127.0.0.1:52061
export SCHEDULER_FRONTEND_ENDPOINT=127.0.0.1:50061

export WORKER_1_FAST_CAS=$NATIVELINK_CACHE_DIR/worker_1_data/cas
export WORKER_2_FAST_CAS=$NATIVELINK_CACHE_DIR/worker_2_data/cas

export RUST_LOG=debug,h2=off,tonic=info,tower=off,hyper=off
export WORKER_1_WORK_DIR=$NATIVELINK_CACHE_DIR/worker_1
export WORKER_2_WORK_DIR=$NATIVELINK_CACHE_DIR/worker_2

if [ -z $REDIS_HOST ]; then
  [[ $(docker ps -al -f name=$CONTAINER_NAME --format '{{.Names}}') == $CONTAINER_NAME ]] || docker run -d --name redis-stack -p 6379:6379 -p 8001:8001 redis/redis-stack:latest
fi

echo "Redis Host - $REDIS_HOST"


if [ $1 = "clean" ]; then
  rm -rf $NATIVELINK_CACHE_DIR
  docker exec -it redis-stack redis-cli flushdb
elif [ $1 = "frontend" ]; then
  cargo run --bin nativelink -- $WORKING_DIR/scheduler_frontend.json
elif [ $1 = "backend" ]; then
  cargo run --bin nativelink -- $WORKING_DIR/scheduler_backend_1.json
elif [ $1 = "tmux" ]; then
  tmux new-session \; \
    new-window "RUST_LOG=debug,h2=off,tonic=info,tower=off,hyper=off,nativelink_service=off cargo run --bin nativelink -- $WORKING_DIR/scheduler_frontend.json" \; \
    split-window -h "cargo run --bin nativelink -- $WORKING_DIR/scheduler_backend_1.json" \; \
    split-window -v "cargo run --bin nativelink -- $WORKING_DIR/scheduler_backend_2.json" \; \
    attach
fi
