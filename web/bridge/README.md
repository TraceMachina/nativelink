# NativeLink Bridge (Experimental)

Make sure you are running an instance of Redis or DragonflyDB in your network.

For DragonflyDB inside docker run:

```bash
docker run \
  -d --name some-dragonfly \
  -p 6379:6379 \
  --ulimit memlock=-1 \
  docker.dragonflydb.io/dragonflydb/dragonfly

```

For Redis inside docker run:

```bash
docker run -d --name some-redis \
  -p 6379:6379 \
  redis
```

Set the Redis URL and the NativeLink pub sub channel ENV variables in `.env` as defined in the `.env.example`


The Redis URL format: 'redis://alice:foobared@awesome.redis.server:6380'

The NativeLink pub sub channel ENV variable should match `experimental_pub_sub_channel` inside `nativelink-config/example/basic_bes.json`.

Make sure you have set also the `key_prefix` in `nativelink-config/example/basic_bes.json`

## You need 4 Components + Redis

### 1. NativeLink

Start an instance of NativeLink with the basic_bes.json inside the `nativelink-config/example/basic_bes.json`.

### 2. NativeLink Web Bridge

Install the dependencies and run the bridge:

```bash
bun i && bun run index.ts
```

### 3. NativeLink Web UI

Inside the web/ui directory run:

```bash
bun i & bun dev
```

Now you can open http://localhost:4321.


### 4. Bazel

Now you can run your Bazel build with NativeLink and see it in real-time going into the web app

Include this in your .bazelrc
```bash
bazel clean && bazel build \
    --remote_cache=grpc://localhost:50051 \
    --remote_executor=grpc://localhost:50051 \
    --bes_backend=grpc://localhost:50061 \
    --bes_results_url=http://localhost:4321/builds \
    --bes_upload_mode=fully_async \
    --build_event_publish_all_actions=true \
    //local-remote-execution/examples:hello_lre
```

Make sure to use the right IP, if it's not hosted on `localhost`


```bash
bazel build some-target
```
