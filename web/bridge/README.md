# NativeLink Bridge

Make sure you are running an instance of Redis or DragonflyDB in your local network.

For DragonflyDB inside docker run:

```bash
docker run -d --name some-dragonfly -p 6379:6379 --ulimit memlock=-1 docker.dragonflydb.io/dragonflydb/dragonfly
```

For Redis inside docker run:

```bash
docker run -d --name some-redis -p 6379:6379 redis
```

## You need 4 Shells to be open

### 1. Shell (NativeLink)

Start an instance of NativeLink and connect it with the basic_bes_conf.json to the redis/dragonflydb (default values):

```bash
cd ../.. && ./result/bin/nativelink ./nativelink-config/examples/basic_bes.json
```

## 2. Shell (NativeLink Web Bridge)

Install dependencies of the bridges:

```bash
bun i
```

Run the Bridge:

```bash
bun run index.ts
```

## 3. Shell (NativeLink Web App)

Inside the web/app directory run:

```bash
bun i & bun dev
```

Now you can open http://localhost:4321/app.


## 4. Shell (Bazel)

Now you can run your Bazel build with NativeLink and see it in real-time going into the web app

Include this in your .bazelrc
```bash
build --remote_instance_name=main
build --remote_cache=grpc://127.0.0.1:50051
build --remote_executor=grpc://127.0.0.1:50051
build --bes_backend=grpc://127.0.0.1:50061
build --bes_results_url=http://127.0.0.1:50061/
build --bes_upload_mode=fully_async
build --build_event_publish_all_actions=true
```

```bash
bazel build some-target
```




This project was created using `bun init` in bun v1.1.25. [Bun](https://bun.sh) is a fast all-in-one JavaScript runtime.
