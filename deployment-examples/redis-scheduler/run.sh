WORKING_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
export NATIVELINK_CACHE_DIR=$WORKING_DIR/.cache/nativelink
export CA=$WORKING_DIR/example-do-not-use-in-prod-rootca.crt
export PEM=$WORKING_DIR/example-do-not-use-in-prod-key.pem

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

if [ $1 = "clean" ]; then
  rm -rf $CAS_PATH $AC_PATH $WORKER_2_WORK_DIR $WORKER_1_WORK_DIR /tmp/nativelink
  docker exec -it my-redis-stack redis-cli flushdb
else
  if [ $1 = "cas" ]; then
    CONFIG_FILE=$WORKING_DIR/local-storage-cas.json
  elif [ $1 = "backend" ]; then
    CONFIG_FILE=$WORKING_DIR/scheduler_backend_$2.json
  elif [ $1 = "frontend" ]; then
    CONFIG_FILE=$WORKING_DIR/scheduler_frontend.json
  elif [ $1 = "worker_1" ]; then
    rm -rf $WORKER_1_FAST_CAS
    CONFIG_FILE=$WORKING_DIR/worker_1.json
  elif [ $1 = "worker_2" ]; then
    rm -rf $WORKER_2_FAST_CAS
    CONFIG_FILE=$WORKING_DIR/worker_2.json
  fi
  cargo run --bin nativelink -- $CONFIG_FILE
fi
