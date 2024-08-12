```
CA=deployment-examples/redis-scheduler/example-do-not-use-in-prod-rootca.crt PEM=deployment-examples/redis-scheduler/example-do-not-use-in-prod-key.pem SCHEDULER_BACKEND_1_ENDPOINT=127.0.0.1:51061 SCHEDULER_BACKEND_2_ENDPOINT=127.0.0.1:52061 SCHEDULER_FRONTEND_ENDPOINT=127.0.0.1:50061 RUST_LOG=debug,h2=off,tonic=info,tower=off,nativelink_service=off,hyper=off cargo run --bin nativelink -- deployment-examples/redis-scheduler/local-storage-cas.json

SCHEDULER_BACKEND_1_ENDPOINT=127.0.0.1:51061 SCHEDULER_BACKEND_2_ENDPOINT=127.0.0.1:52061 SCHEDULER_FRONTEND_ENDPOINT=127.0.0.1:50061 RUST_LOG=debug,h2=off,tonic=info,tower=off,nativelink_service=off,hyper=off cargo run --bin nativelink -- deployment-examples/redis-scheduler/scheduler_frontend.json

SCHEDULER_BACKEND_1_ENDPOINT=127.0.0.1:51061 SCHEDULER_BACKEND_2_ENDPOINT=127.0.0.1:52061 SCHEDULER_FRONTEND_ENDPOINT=127.0.0.1:50061 RUST_LOG=debug,h2=off,tonic=info,tower=off,nativelink_service=off,hyper=off cargo run --bin nativelink -- deployment-examples/redis-scheduler/scheduler_backend_1.json

SCHEDULER_BACKEND_2_ENDPOINT=127.0.0.1:51061 SCHEDULER_BACKEND_2_ENDPOINT=127.0.0.1:52061 SCHEDULER_FRONTEND_ENDPOINT=127.0.0.1:50061 RUST_LOG=debug,h2=off,tonic=info,tower=off,nativelink_service=off,hyper=off cargo run --bin nativelink -- deployment-examples/redis-scheduler/scheduler_backend_2.json

SCHEDULER_BACKEND_1_ENDPOINT=127.0.0.1:51061 SCHEDULER_BACKEND_2_ENDPOINT=127.0.0.1:52061 SCHEDULER_FRONTEND_ENDPOINT=127.0.0.1:50061 RUST_LOG=debug,h2=off,tonic=info,tower=off,nativelink_service=off,hyper=off cargo run --bin nativelink -- deployment-examples/redis-scheduler/worker_1.json

SCHEDULER_BACKEND_1_ENDPOINT=127.0.0.1:51061 SCHEDULER_BACKEND_2_ENDPOINT=127.0.0.1:52061 SCHEDULER_FRONTEND_ENDPOINT=127.0.0.1:50061 RUST_LOG=debug,h2=off,tonic=info,tower=off,nativelink_service=off,hyper=off cargo run --bin nativelink -- deployment-examples/redis-scheduler/worker_2.json
```
