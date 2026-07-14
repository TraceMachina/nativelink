# AGENTS.md

A machine-readable map of the NativeLink repository for AI coding agents and new
contributors. NativeLink is a high-performance remote build cache and execution
platform (Remote Execution API), written in Rust.

Full documentation: https://docs.nativelink.com. Link index for machine readers:
https://docs.nativelink.com/llms.txt (full corpus at `/llms-full.txt`). Human
contributor guide: [`CONTRIBUTING.md`](CONTRIBUTING.md).

## The four roles

NativeLink is four cooperating roles that speak the Remote Execution API:

- **CAS** — content-addressable storage for build inputs/outputs, keyed by digest.
- **Action Cache (AC)** — maps an action's digest to its cached result.
- **Scheduler** — queues actions and matches them to workers by platform property.
- **Workers** — execute actions and stream results back to the CAS.

A single binary can run any combination of these, configured in JSON5.

## Crate map (where things live)

| Crate | Owns |
| ----- | ---- |
| `nativelink` (root, `src/bin/nativelink.rs`) | The server binary; wires config to running services. |
| `nativelink-config` | JSON5 config schema. Stores in `src/stores.rs`, server/worker in `src/cas_server.rs`. Source of truth for the config reference. |
| `nativelink-service` | gRPC services: CAS, AC, Execution, Capabilities, ByteStream, Worker API, BEP, health, fetch/push. |
| `nativelink-store` | Every store implementation: filesystem, memory, redis, S3/R2/GCS/Azure/OCI/Mongo, and the composition stores (`fast_slow`, `shard`, `size_partitioning`, `compression`, `dedup`, `existence_cache`, `verify`, `ref`). |
| `nativelink-scheduler` | Scheduler internals: matching engine, awaited-action DB, worker registry, state manager, retries. |
| `nativelink-worker` | Worker: action execution, input materialization, sandboxing/namespaces, directory cache. |
| `nativelink-util` | Shared utilities: `fs`/`fs_util`, `evicting_map`, `action_messages`, digests, and the OTLP metrics (`metrics.rs`). |
| `nativelink-proto` | Generated protobufs (REAPI + NativeLink extensions). |
| `nativelink-metric` / `nativelink-macro` | The `#[metric]` component-metrics derive system and proc macros. |
| `nativelink-error` | The `Error` type and `ResultExt`. |
| `nativelink-test` / `nativelink-redis-tester` | Test harness (`nativelink_test`) and Redis test tooling. |

## Where to change what

- **Add or change a config field** — edit the relevant spec in
  `nativelink-config/src/stores.rs` or `cas_server.rs`; the config reference is
  generated from the doc comments, so document the field there.
- **Add a store type** — implement `StoreDriver` in `nativelink-store/src`, add
  its spec to `nativelink-config/src/stores.rs`, and wire it in the store factory.
- **Add or change a gRPC service** — `nativelink-service/src/*_server.rs`.
- **Change scheduling/matching/retries** — `nativelink-scheduler/src`.
- **Change action execution / worker behavior** —
  `nativelink-worker/src/running_actions_manager.rs`.
- **Add an OTLP metric** — declare and record it in
  `nativelink-util/src/metrics.rs`; record it at the call site; add it to the
  metrics catalog docs. Do not leave a metric declared-but-never-recorded.

## Changing X: source of truth, and the doc that must follow

When a change lands, this table says what else has to move with it. The rule is
that the source of truth is always in the repo, and the doc either regenerates
from it or cites it — never restates it by hand.

| You changed | Source of truth | Doc that must follow |
| ----------- | --------------- | -------------------- |
| A config field or its doc comment | `nativelink-config/src/{stores,cas_server}.rs` | `reference/nativelink-config/*` — regenerate, never hand-edit |
| A store's behavior or defaults | `nativelink-store/src/*` | `reference/store-reference` (generated) + the backend page under `how-to/stores/` |
| A metric name, type, or label | `nativelink-util/src/metrics.rs` + its call sites | `reference/metrics-reference` (generated) + `operate/observability` |
| A CLI flag or `NL_*` env var | the binary's arg parsing | `reference/cli-and-env` |
| A gRPC service or its surface | `nativelink-service/src/*_server.rs` | `reference/protocol-api` |
| Scheduler matching or platform-property semantics | `nativelink-scheduler/src` | `remote-execution/platform-properties` + `explanations/scheduler-internals` |
| Worker sandboxing or input materialization | `nativelink-worker/src/running_actions_manager.rs` | `explanations/worker-execution` |
| The canonical production config | `deployment-examples/`, `nativelink-config/examples/` | `operate/production-config` — the snippets are lifted from there, not invented |
| A page's URL | `web/apps/docs/content/docs/**/meta.json` | add a redirect in `web/apps/docs/next.config.mjs` |

## Build, test, verify

NativeLink builds with both Bazel and Cargo.

```bash
bazel test //...                                  # all tests (first run 10-20 min)
bazel test //nativelink-store/tests:s3_store_test # one target
bazel build //nativelink:nativelink               # the server binary
cargo test -p nativelink-store                    # a single crate with cargo
```

Run the built server against a config:

```bash
bazel run //nativelink:nativelink -- ./path/to/config.json5
```

Example configs live in `nativelink-config/examples/` and runnable deployments in
`deployment-examples/` (docker-compose, including a multi-worker set) and
`integration_tests/`.

## The docs, and their gates

The docs site is a [Fumadocs](https://fumadocs.dev) app at `web/apps/docs`. Pages
are MDX under `content/docs/`, and `meta.json` in each directory controls both the
sidebar order and which pages are published.

```bash
cd web && bun install
bun run --filter docs dev     # local docs server
bun run --filter docs build   # what CI builds
bunx biome check --write .    # lint and format
```

Four things a docs change has to satisfy:

- **Vale and typos** run in pre-commit. New product nouns go in
  `.github/styles/config/vocabularies/TraceMachina/accept.txt`.
- **Biome** formats and lints the TypeScript and the MDX components.
- **Every nav entry resolves.** A `meta.json` entry naming a page that doesn't
  exist is a broken sidebar link.
- **Moved URLs redirect.** Agents cache URLs longer than humans keep bookmarks;
  a 404 on an old path is a regression.

Page structure follows four archetypes — tutorial, how-to, explanation,
reference — with a template for each in `web/apps/docs/templates/`. The
conventions those templates encode:

- A narrative page never inlines an exhaustive field list. It explains the
  fields that carry a decision and links the generated reference for the rest.
- Pages in the CAS → RBE ladder open with `<Rung>`, so a reader landing cold
  from a search knows what the page assumes.
- Tutorials and how-tos close with `<VerifyBlock>`: a checkable claim, not "it
  should work now".
- Behavioural claims carry `<SourceLink>`, permalinked to a release tag rather
  than to `main`. The pinned ref lives in `web/apps/docs/lib/source-ref.ts`; bump
  it in the same change that regenerates the reference for a new release.

## Conventions

- Pre-commit runs rustfmt, `typos`, and (for docs) `vale`. Write to pass them.
- The config reference under `web/apps/docs/content/docs/reference/nativelink-config`
  is autogenerated via the `build-schema` binary — regenerate, never hand-edit.
- Prefer generating docs/reference from a code source of truth over hand-writing,
  to prevent drift.
