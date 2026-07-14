# AGENTS.md

A machine-readable map of the NativeLink repository for AI coding agents and new
contributors. NativeLink is a high-performance remote build cache and execution
platform (Remote Execution API), written in Rust.

Full documentation: https://docs.nativelink.com. Two entry points for machine
readers, both generated from the docs navigation so neither can drift from the
sidebar: https://docs.nativelink.com/llms.txt is the link index, one line per
page in reading order, and https://docs.nativelink.com/llms-full.txt is the same
corpus with page bodies inlined. Human contributor guide:
[`CONTRIBUTING.md`](CONTRIBUTING.md).

## The four roles

NativeLink is four cooperating roles that speak the Remote Execution API:

- **CAS**: content-addressable storage for build inputs/outputs, keyed by digest.
- **Action Cache (AC)**: maps an action's digest to its cached result.
- **Scheduler**: queues actions and matches them to workers by platform property.
- **Workers**: execute actions and stream results back to the CAS.

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

- **Add or change a config field**: edit the relevant spec in
  `nativelink-config/src/stores.rs` or `cas_server.rs`; the config reference is
  generated from the doc comments, so document the field there.
- **Add a store type**: implement `StoreDriver` in `nativelink-store/src`, add
  its spec to `nativelink-config/src/stores.rs`, and wire it in the store factory.
- **Add or change a gRPC service**: `nativelink-service/src/*_server.rs`.
- **Change scheduling/matching/retries**: `nativelink-scheduler/src`.
- **Change action execution / worker behavior**:
  `nativelink-worker/src/running_actions_manager.rs`.
- **Add an OTLP metric**: declare and record it in
  `nativelink-util/src/metrics.rs`; record it at the call site; regenerate the
  metrics reference. Do not leave a metric declared-but-never-recorded: the
  generated reference has a column for exactly that, and it will say so.

## Changing X: source of truth, and the doc that must follow

When a change lands, this table says what else has to move with it. The rule is
that the source of truth is always in the repo, and the doc either regenerates
from it or cites it, never restates it by hand. Every path in the right-hand
column is a real page under `web/apps/docs/content/docs/`.

| You changed | Source of truth | Doc that must follow |
| ----------- | --------------- | -------------------- |
| A config field or its doc comment | `nativelink-config/src/{stores,cas_server}.rs` | `reference/nativelink-config/*`; regenerate with `gen:config-reference`, never hand-edit |
| A store's behavior or defaults | `nativelink-store/src/*` | the backend page under `how-to/stores/`, plus `reference/nativelink-config/store-overview` if the composition model moved |
| A metric name, type, or label | `nativelink-util/src/metrics.rs` and its call sites | `reference/metrics` (regenerate with `gen:metrics-reference`) and `operate/observability` |
| A CLI flag or `NL_*` env var | the binary's arg parsing | `reference/cli-and-env` |
| A gRPC service or the set of services a server exposes | `nativelink-service/src/*_server.rs` | `configuration/servers-and-services`, which names every service as the config spells it |
| Scheduler matching or platform-property semantics | `nativelink-scheduler/src` | `remote-execution/platform-properties`, and `explanations/architecture` if the model changed rather than the mechanics |
| Worker sandboxing or input materialization | `nativelink-worker/src/running_actions_manager.rs` | `explanations/architecture` and `operate/security-hardening`; the second one states what the sandbox is *not* |
| The licence header on a file, or `LICENSE` | the headers themselves | `reference/oss-and-enterprise`, which is the only place the licence split is explained |
| The canonical production config | `deployment-examples/`, `nativelink-config/examples/` | `operate/production-config`; the snippets are lifted from there, not invented |
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
bun run --filter docs dev     # local docs server; regenerates first
bun run --filter docs build   # what CI builds
bunx biome check --write .    # lint and format

bun --filter @nativelink/docs lint:snippets  # JSON5 snippets against the config schema
bun --filter @nativelink/docs lint:anchors   # explicit anchors on headings and FAQ entries
```

Six things a docs change has to satisfy:

- **Vale and typos** run in pre-commit. New product nouns go in
  `.github/styles/config/vocabularies/TraceMachina/accept.txt`.
- **Biome** formats and lints the TypeScript and the MDX components.
- **Every nav entry resolves.** A `meta.json` entry naming a page that doesn't
  exist is a broken sidebar link, and `gen:llms` fails loudly on one.
- **Moved URLs redirect.** Agents cache URLs longer than humans keep bookmarks;
  a 404 on an old path is a regression.
- **`lint:snippets` passes.** Every key in a JSON5 config snippet has to exist in
  the generated config reference. A snippet that is deliberately wrong (showing
  a mistake, or a foreign tool's config) opts out with
  `{/* lint-snippets: ignore */}` above it.
- **`lint:anchors` passes.** Every heading and every `<Accordion>` carries an
  explicit anchor, so rewording a heading cannot silently repoint a citation.
  `lint:anchors --fix` writes the anchor Fumadocs would have derived anyway,
  which means running it never moves an existing link.

Four things under `web/apps/docs` are generated. Regenerate them; never hand-edit:

| Generated | From | Command |
| --------- | ---- | ------- |
| `content/docs/reference/nativelink-config/*` | the `nativelink-config` crate, via `build-schema` | `gen:config-reference` |
| `content/docs/reference/metrics.mdx` | `nativelink-util/src/metrics.rs` and its call sites | `gen:metrics-reference` |
| `content/docs/reference/changelog.md` | the repository-root `CHANGELOG.md` | `gen:changelog` |
| `public/llms.txt`, `public/llms-full.txt` | the navigation and page frontmatter | `gen:llms` |

The last two are gitignored and rebuilt by `dev` and `build`, so they cannot be
committed in a stale state.

Page structure follows four archetypes (tutorial, how-to, explanation,
reference), with a template for each in `web/apps/docs/templates/`. The
conventions those templates encode:

- A narrative page never inlines an exhaustive field list. It explains the
  fields that carry a decision and links the generated reference for the rest.
- Pages on the reading path (Getting started, Remote execution, Configuration,
  How-to guides, plus Why NativeLink before and Operate after) open with
  `<Prerequisites>`, so a reader landing cold from a search knows what the page
  assumes.
- Tutorials and how-tos close with `<VerifyBlock>`: a checkable claim, not "it
  should work now".
- Behavioural claims carry `<SourceLink>`, permalinked to a release tag rather
  than to `main`. The pinned ref lives in `web/apps/docs/lib/source-ref.ts`; bump
  it in the same change that regenerates the reference for a new release.
- Headings carry an explicit `[#anchor]`, and `<Accordion>` entries an `id`, so
  a citation to a specific claim keeps resolving after the prose is reworded.

## Conventions

- Pre-commit runs rustfmt, `typos`, and (for docs) `vale`. Write to pass them.
- The config reference under `web/apps/docs/content/docs/reference/nativelink-config`
  is autogenerated via the `build-schema` binary; regenerate, never hand-edit.
- Prefer generating docs/reference from a code source of truth over hand-writing,
  to prevent drift.
