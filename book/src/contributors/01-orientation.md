# Orientation: The Shape of the Codebase

You cloned the repo. `ls` printed a dozen `nativelink-*` directories, a `src/`, two lockfiles, a `flake.nix`, and a wall of dotfiles. This chapter is the map. By the end you should be able to point at any crate and say what it is, which way the dependencies flow, and where to go to change a given thing — without grepping blind.

NativeLink is a Cargo workspace with a Bazel build layered on top (Chapter 2 covers the dual build). The workspace root is a *binary* crate named `nativelink`; everything else is a library crate it composes. The root manifest lists eight path dependencies — the libraries that make up the server — and nothing else of substance (`Cargo.toml:33-41`).

## The Crate Map

The crates fall into three tiers: **foundations** (no NativeLink dependencies, or nearly so), **the store/scheduler/worker engine**, and **the service and binary** on top. Walk them bottom-up and the whole thing makes sense.

### Foundations

**`nativelink-proto`** — the generated protocol. This is REAPI and friends, compiled from `.proto` into Rust. The library is literally the generated tree: its lib path points at `genproto/lib.rs`, which re-exports `build::bazel::remote::execution::v2` (the Remote Execution API), `build::bazel::remote::asset::v1` (Remote Asset), the `google::bytestream`, `google::longrunning`, and `google::rpc` standard services, and NativeLink's own `com::github::trace_machina::nativelink::remote_execution` module for the worker API and origin events (`nativelink-proto/genproto/lib.rs:38-95`). You touch this when you change a wire surface — a new RPC, a new field — and the hand-written NativeLink protos live under `nativelink-proto/com/github/trace_machina/nativelink/remote_execution/` (e.g. `worker_api.proto`, `events.proto`). It depends only on `prost`/`tonic`, nothing NativeLink.

**`nativelink-error`** — the one error type. `Error` (`nativelink-error/src/lib.rs:77`) carries a gRPC-style `Code` (`CodeDef`, `nativelink-error/src/lib.rs:603`) plus a message stack, and the `make_err!` / `make_input_err!` macros (`nativelink-error/src/lib.rs:28-43`) construct them. The pervasive `.err_tip(|| ...)` you see everywhere comes from here — it pushes context onto the stack without discarding the original. Every crate returns `Result<_, Error>`. You touch this rarely, and when you do it's to add a conversion or an error code.

**`nativelink-metric`** and **`nativelink-metric-macro-derive`** — the observability substrate. `nativelink-metric` defines the `MetricsComponent` derive and the publishing machinery (`nativelink-metric/src/lib.rs:44-49`); the `#[metric]` field attribute you saw on the `Store` wrapper in Part III is this crate. You touch it when you want a struct's internals to show up in the metrics/introspection output.

**`nativelink-macro`** — one proc-macro: `#[nativelink_test]` (`nativelink-macro/src/lib.rs:36-99`). It is the test harness attribute used across the whole workspace. Chapter 2 dissects exactly what it expands to; for now, know that every unit test is annotated with it rather than `#[tokio::test]` directly, and this crate is where that expansion lives.

**`nativelink-config`** — the config schema. Every JSON5 config file deserializes into the structs here: `cas_server` (servers, services, workers), `stores` (the store algebra), `schedulers`, plus `serde_utils` and `backcompat` (`nativelink-config/src/lib.rs:15-19`). This crate is the single source of truth for what a valid config *is*. You touch it whenever you add a store type, a scheduler knob, or a server option — the Rust struct and its serde attributes here are the contract the operator writes against. It depends only on `nativelink-error`.

### The Engine

**`nativelink-util`** — the shared library that everything above depends on, and the home of the single most important abstraction in the system: the store trait. `StoreLike`, `StoreDriver`, and `StoreKey` all live in `nativelink-util/src/store_trait.rs` (traits at lines 435 and 621; `StoreKey` at line 195). It also owns `buf_channel` (the backpressure-aware streaming channels — `DropCloserWriteHalf` at `buf_channel.rs:63`, `DropCloserReadHalf` at line 214), `task` (the spawn wrappers you *must* use instead of `tokio::spawn` — see Chapter 2), `digest_hasher`, `health_utils`, `action_messages`, and `fs`/`fs_util` (`nativelink-util/src/lib.rs:15-42`). You will touch `nativelink-util` more than any other crate that isn't the one you came to change, because the primitives everyone shares live here.

**`nativelink-store`** — every storage backend and every composition wrapper, each one a `StoreDriver` implementation in its own file: `filesystem_store.rs`, `memory_store.rs`, `s3_store.rs`, `gcs_store.rs`, `azure_blob_store.rs`, `redis_store.rs`, `mongo_store.rs`, `grpc_store.rs` for leaves; `fast_slow_store.rs`, `compression_store.rs`, `dedup_store.rs`, `shard_store.rs`, `size_partitioning_store.rs`, `ref_store.rs`, `verify_store.rs`, `existence_cache_store.rs`, `completeness_checking_store.rs`, `noop_store.rs` for composition. The `default_store_factory.rs` turns a config `StoreSpec` into a live `Store`, and `store_manager.rs` is the registry that holds them by name (`nativelink-store/src/lib.rs:15-45`). You touch this crate to add or fix a backend, or to change how a wrapper composes.

**`nativelink-scheduler`** — the brain that matches actions to workers. `SimpleScheduler` is the default (`nativelink-scheduler/src/simple_scheduler.rs:173`); around it sit `api_worker_scheduler.rs`, `cache_lookup_scheduler.rs`, `property_modifier_scheduler.rs`, `grpc_scheduler.rs`, the state managers (`simple_scheduler_state_manager.rs`), and the awaited-action databases (`memory_awaited_action_db.rs`, `store_awaited_action_db.rs`). `default_scheduler_factory.rs` builds the right pair from config (`nativelink-scheduler/src/lib.rs:15-35`). You touch this crate to change a *scheduling decision* — matching, retries, platform-property handling, dispatch.

**`nativelink-worker`** — the executor side. `LocalWorker` (`nativelink-worker/src/local_worker.rs:768`) connects to a scheduler and runs actions; `running_actions_manager.rs` is the enormous heart of it (the `RunningActionsManager` and `RunningActionImpl` that stage inputs, exec, and collect outputs — `running_actions_manager.rs:1681,2928`); `directory_cache.rs`, `namespace_utils.rs` (Linux sandboxing), and `persistent_worker/` round it out (`nativelink-worker/src/lib.rs:15-25`). You touch this crate to change what happens *when an action actually executes* on a machine.

### Service and Binary

**`nativelink-service`** — the gRPC/HTTP server surface. One module per service: `ac_server.rs` (`AcServer`, line 45), `cas_server.rs` (`CasServer`, line 174), `bytestream_server.rs`, `execution_server.rs` (`ExecutionServer`, line 280), `capabilities_server.rs`, `worker_api_server.rs`, `bep_server.rs`, `fetch_server.rs`, `health_server.rs`, `push_server.rs` (`nativelink-service/src/lib.rs:15-26`). Each wraps a `Store` or a scheduler handle and implements the corresponding tonic service trait. You touch this crate — together with `nativelink-proto` — to change a *gRPC surface*: new RPC behavior, request validation, how a protocol call maps onto store or scheduler operations.

**`nativelink` (the root binary)** — `src/bin/nativelink.rs`. This is the entrypoint and the wiring. It has almost no business logic; it reads a config, builds everything the config names, and runs it. More on that below.

### `nativelink-test` is not a crate

`nativelink-test/` holds exactly one thing: a `fuzz/` subdirectory (`nativelink-test/fuzz`), which is a standalone `cargo-fuzz` package named `nativelink-fuzz` and is *excluded* from the workspace (`Cargo.toml:3-7`). There is no `nativelink-test` library. **Unit and integration tests live inside each crate**, under `<crate>/tests/` and in `#[cfg(test)]` modules — e.g. `nativelink-store/tests/`. The fuzzer is covered in Chapter 2.

> One caveat on this checkout: the branch (`pr/keeper-ffi`) adds `nativelink-keeper` and `nativelink-redis-tester` directories that are not part of upstream `main`. Ignore them — nothing in this chapter depends on them, and the eight crates above are the ones that match upstream.

## Which Way the Dependencies Flow

Dependencies point *down*. The foundations (`proto`, `error`, `metric`, `macro`, `config`) depend on nothing NativeLink-internal of consequence; `config` depends on `error`, `error` depends on `metric` and `proto`. `util` sits on top of those. `store` depends on `util` (and the foundations). `scheduler` and `worker` each depend on `store` and `util`. `service` depends on `scheduler`, `store`, and `util`. The binary depends on all of them (`Cargo.toml:34-41`).

```
                    nativelink  (src/bin/nativelink.rs)
                        │
                 nativelink-service
                   │        │
        nativelink-scheduler │   (worker is wired in by the binary,
                   │        │    not by service)
            nativelink-store ──────── nativelink-worker
                        │
                 nativelink-util
                        │
   nativelink-config  nativelink-proto  nativelink-error  nativelink-metric
```

The practical consequence: a change low in the stack (say, a `StoreKey` signature in `nativelink-util`) ripples up through store, scheduler, worker, and service and makes everything recompile. A change high in the stack (a new field in `cas_server.rs`) touches almost nothing below it. Build and test from the crate you changed *upward*, never the reverse.

## From a Config File to a Running Server

The entrypoint is `src/bin/nativelink.rs`. The flow is short and worth tracing once:

1. **Parse args, load config.** `get_config()` parses the CLI with clap and deserializes the named file: `CasConfig::try_from_json5_file(&args.config_file)` (`src/bin/nativelink.rs:857-860`). A config file *is* a `CasConfig` — the struct from `nativelink-config`.

2. **Set global state.** `main` builds the Tokio multi-thread runtime, initializes tracing, applies the `GlobalConfig` (open-file limit, default digest hash function, health-check sizing) and hands off to `inner_main` (`src/bin/nativelink.rs:862-910`).

3. **Build the stores.** `inner_main` creates a `StoreManager`, then loops over `cfg.stores`, calling `store_factory(&spec, ...)` for each named spec and registering the result by name; afterward it runs `store_manager.run_post_init()` so stores that reference other stores (e.g. `RefStore`) can resolve them (`src/bin/nativelink.rs:219-234`). This two-phase init is why config order doesn't matter — see Part III.

4. **Build the schedulers.** It loops over `cfg.schedulers`, calling `scheduler_factory(spec, &store_manager, ...)` and sorting the results into action-schedulers and worker-schedulers maps (`src/bin/nativelink.rs:272-283`).

5. **Build the servers.** For each entry in `cfg.servers` it assembles a tonic `Routes` and `add_service`s the enabled gRPC services — `CasServer::new(cfg, &store_manager, ...)` and its siblings — binding them to the configured listener (`src/bin/nativelink.rs:285-333`, service construction around `cas_server.rs`).

6. **Build the workers.** If the config names local workers, `new_local_worker(...)` is spawned to connect back to a worker-scheduler and start executing.

The whole binary is a factory driven by data. There is no hardcoded topology: the store graph, the scheduler, and the service set are all assembled from the config at startup. That is the design the rest of this book keeps returning to.

## The Repo's Own Orientation Docs

Before you go deep, read these — they are maintained by the project and will outlive this chapter:

- **`AGENTS.md`** — the operational cheat sheet. It lists the canonical build/test commands (`bazel test //...`, `cargo test -p <crate>`, `bazel build //nativelink:nativelink`) and the pre-commit expectations (`AGENTS.md:118-121,200`). Read it first.
- **`CONTRIBUTING.md`** — the full contributor process: formatting (`bazel run --config=rustfmt @rules_rust//:rustfmt`, `CONTRIBUTING.md:311`), pre-commit (`pre-commit run -a`, line 320), docs builds, and the AI-assistance disclosure policy (lines 18-33).
- **`llms.txt`** / `llms-small.txt` / `llms-full.txt` — machine-readable digests of the codebase at the repo root, intended for feeding to tools. `llms.txt` is the index; the `-full` variant is the whole thing inlined.
- **`clippy.toml`**, **`.rustfmt.toml`**, **`Cargo.toml` lints** — the enforced style and lint rules. Chapter 2 explains why these, not `cargo test`, are what actually gate your PR.

## Where Do I Go to Change X

| You want to change… | Go to | Likely file(s) |
|---|---|---|
| How a storage backend behaves (S3, filesystem, Redis…) | `nativelink-store` | `nativelink-store/src/<backend>_store.rs` |
| How stores compose (tiering, compression, sharding, dedup) | `nativelink-store` | `fast_slow_store.rs`, `compression_store.rs`, `shard_store.rs`, `dedup_store.rs` |
| The store trait itself | `nativelink-util` | `nativelink-util/src/store_trait.rs` |
| A scheduling decision (matching, retries, dispatch) | `nativelink-scheduler` | `simple_scheduler.rs`, `simple_scheduler_state_manager.rs` |
| What happens when an action executes | `nativelink-worker` | `running_actions_manager.rs`, `local_worker.rs` |
| A gRPC surface (new RPC, request handling) | `nativelink-service` **+** `nativelink-proto` | `nativelink-service/src/<svc>_server.rs`, `nativelink-proto/.../*.proto` |
| The config schema (a new knob or store type) | `nativelink-config` | `cas_server.rs`, `stores.rs`, `schedulers.rs` |
| The error type or an error code | `nativelink-error` | `nativelink-error/src/lib.rs` |
| Shared primitives (streaming, spawning, hashing, fs) | `nativelink-util` | `buf_channel.rs`, `task.rs`, `digest_hasher.rs`, `fs.rs` |
| Metrics / introspection on a struct | `nativelink-metric` (derive) | add `#[derive(MetricsComponent)]` + `#[metric]` |
| How the server boots / wires config → runtime | the root binary | `src/bin/nativelink.rs` |
| The test harness macro | `nativelink-macro` | `nativelink-macro/src/lib.rs` |

If a change spans the wire *and* the behavior behind it — the common case for a new feature — expect to touch `nativelink-proto` first, then `nativelink-service`, then whichever engine crate implements the behavior. Build upward from there.

**Source for this chapter:** the crate manifests under each `nativelink-*/Cargo.toml`, the module roots at each `nativelink-*/src/lib.rs`, and the entrypoint [`src/bin/nativelink.rs`](https://github.com/TraceMachina/nativelink/blob/main/src/bin/nativelink.rs).
