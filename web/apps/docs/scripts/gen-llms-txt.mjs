#!/usr/bin/env node
// Generate public/llms.txt (https://llmstxt.org) from the repository's own
// sources so the machine-readable docs map for LLM agents can't drift:
//
//   - the current version + date come from the root CHANGELOG.md (the same
//     file gen-changelog.mjs consumes),
//   - the store list comes from the config crate's JSON Schema (the `StoreSpec`
//     enum), built with the same `build-schema` binary gen-config-reference.mjs
//     uses — cached to lib/store-specs.json so ordinary builds need no cargo,
//   - the sample config is inlined verbatim from
//     nativelink-config/examples/basic_cas.json5 (syntax-checked by tests),
//   - the "more examples" list is read from nativelink-config/examples/,
//   - the documentation table of contents is read from the docs content tree
//     (content/docs + its meta.json ordering).
//
// Only a short editorial header (the intro, build systems, key concepts, crate
// map) is hardcoded. Everything a reviewer flagged as drift-prone is derived.
//
// Usage from web/:
//   bun --filter @nativelink/docs gen:llms-txt              # uses cached store list
//   bun --filter @nativelink/docs gen:llms-txt -- --schema  # rebuild stores

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const docsAppDir = join(here, ".."); // web/apps/docs
const contentRoot = join(docsAppDir, "content/docs");
const storeSnapshot = join(docsAppDir, "lib/store-specs.json");
const outFile = join(docsAppDir, "public/llms.txt");

const DOCS_BASE = "https://docs.nativelink.com";
const GITHUB_TREE =
  "https://github.com/TraceMachina/nativelink/tree/main/nativelink-config/examples";

const wantSchema = process.argv.includes("--schema");

// CHANGELOG.md and nativelink-config/examples live at the repository root,
// ABOVE the web/ workspace. Local checkouts have them on disk, so we walk up to
// find the root (mirrors gen-changelog). Vercel only mounts the web/ workspace
// into the build container, so there the root is absent and we fetch each file
// from GitHub raw pinned to the exact deployed commit — same bytes, no drift.
function findLocalRepoRoot() {
  for (let dir = here; ; ) {
    if (existsSync(join(dir, "cliff.toml")) && existsSync(join(dir, "CHANGELOG.md"))) {
      return dir;
    }
    const parent = resolve(dir, "..");
    if (parent === dir) return null;
    dir = parent;
  }
}

const repoRoot = findLocalRepoRoot();

function deployedCommitRef() {
  const owner = process.env.VERCEL_GIT_REPO_OWNER;
  const repo = process.env.VERCEL_GIT_REPO_SLUG;
  const sha = process.env.VERCEL_GIT_COMMIT_SHA;
  return owner && repo && sha ? { owner, repo, sha } : null;
}

function repoRootUnavailable(what) {
  return new Error(
    [
      `gen-llms-txt: ${what} not found above ${here}, and the VERCEL_GIT_*`,
      "variables needed to fetch it for the deployed commit are unset.",
      "Run from a nativelink checkout, or ensure the deploy exposes VERCEL_GIT_*.",
    ].join(" "),
  );
}

/** Read a repo-root-relative text file: local checkout first, else GitHub raw. */
async function readRepoText(relPath) {
  if (repoRoot) return readFileSync(join(repoRoot, relPath), "utf8");
  const ref = deployedCommitRef();
  if (!ref) throw repoRootUnavailable(relPath);
  const url = `https://raw.githubusercontent.com/${ref.owner}/${ref.repo}/${ref.sha}/${relPath}`;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`gen-llms-txt: fetching ${url} failed: HTTP ${res.status}`);
  console.log(`gen-llms-txt: read ${relPath} from ${url}`);
  return res.text();
}

/** List *.json5 example names: local readdir first, else GitHub contents API. */
async function listExampleNames() {
  const rel = "nativelink-config/examples";
  let names;
  if (repoRoot) {
    names = readdirSync(join(repoRoot, rel));
  } else {
    const ref = deployedCommitRef();
    if (!ref) throw repoRootUnavailable(rel);
    const url = `https://api.github.com/repos/${ref.owner}/${ref.repo}/contents/${rel}?ref=${ref.sha}`;
    const res = await fetch(url, { headers: { "User-Agent": "gen-llms-txt" } });
    if (!res.ok) throw new Error(`gen-llms-txt: listing ${url} failed: HTTP ${res.status}`);
    names = (await res.json()).map((entry) => entry.name);
  }
  return names
    .filter((f) => f.endsWith(".json5"))
    .map((f) => f.replace(/\.json5$/, ""))
    .sort();
}

// (A) Version + date from the top CHANGELOG.md release heading.
async function latestRelease() {
  const changelog = await readRepoText("CHANGELOG.md");
  const m = /^##\s*\[(\d+\.\d+\.\d+(?:-[\w.]+)?)\].*?-\s*(\d{4}-\d{2}-\d{2})/m.exec(changelog);
  if (!m) throw new Error("Could not parse latest release from CHANGELOG.md");
  return { version: m[1], date: m[2] };
}

// (B) Store list from the config crate's JSON Schema (StoreSpec enum).
function firstParagraph(text) {
  if (!text) return "";
  return text
    .split(/\n\s*\n/)[0]
    .replace(/\s+/g, " ")
    .trim();
}

function storesFromSchema(schema) {
  const defs = schema.$defs || schema.definitions || {};
  const storeSpec = defs.StoreSpec;
  if (!storeSpec || !Array.isArray(storeSpec.oneOf)) {
    throw new Error("StoreSpec union not found in schema");
  }
  return (
    storeSpec.oneOf
      .map((branch) => {
        const key = branch.required?.[0] ?? Object.keys(branch.properties ?? {})[0] ?? null;
        if (!key) return null;
        // Keys ordered to match the pretty-format-json pre-commit hook (which
        // sorts object keys) so a `--schema` regen doesn't get reformatted.
        return { description: firstParagraph(branch.description), key };
      })
      .filter(Boolean)
      // Sort by key for a stable, alphabetical order instead of the arbitrary
      // StoreSpec enum declaration order.
      .sort((a, b) => a.key.localeCompare(b.key))
  );
}

function buildSchemaWithCargo() {
  if (!repoRoot) {
    throw new Error("gen-llms-txt: --schema requires a local checkout (no repo root found).");
  }
  const schemaPath = join(repoRoot, "web", "apps", "docs", "lib", "schema.json");
  const schema = JSON.parse(readFileSync(schemaPath, "utf8"));
  return schema;
}

function resolveStores() {
  if (wantSchema || !existsSync(storeSnapshot)) {
    const stores = storesFromSchema(buildSchemaWithCargo());
    mkdirSync(dirname(storeSnapshot), { recursive: true });
    writeFileSync(
      storeSnapshot,
      `${JSON.stringify(
        {
          _comment:
            "AUTOGENERATED by scripts/gen-llms-txt.mjs --schema from the StoreSpec enum. Do not edit by hand.",
          stores,
        },
        null,
        2,
      )}\n`,
    );
    return stores;
  }
  return JSON.parse(readFileSync(storeSnapshot, "utf8")).stores;
}

// (D) Sample config from nativelink-config/examples/basic_cas.json5.
async function sampleConfig() {
  return (await readRepoText("nativelink-config/examples/basic_cas.json5")).trimEnd();
}

// (F) Documentation table of contents from the docs content tree.
function readFrontmatter(file) {
  const text = readFileSync(file, "utf8");
  if (!text.startsWith("---")) return {};
  const end = text.indexOf("\n---", 3);
  if (end === -1) return {};
  const out = {};
  for (const line of text.slice(3, end).split("\n")) {
    const m = /^([A-Za-z_]+):\s*(.*)$/.exec(line.trim());
    if (m) out[m[1]] = m[2].replace(/^["']|["']$/g, "").trim();
  }
  return out;
}

function readMeta(dir) {
  const f = join(dir, "meta.json");
  return existsSync(f) ? JSON.parse(readFileSync(f, "utf8")) : { pages: [] };
}

// A meta.json entry can be a page (`<dir>/<entry>.mdx|.md`) or a subfolder
// (`<dir>/<entry>/` with its own index + meta). Separators look like `---X---`.
function routeFor(absFile) {
  const rel = absFile.slice(contentRoot.length).replace(/\\/g, "/");
  const noExt = rel.replace(/\.(mdx?|md)$/i, "").replace(/\/index$/i, "");
  return noExt === "" ? "/" : noExt;
}

function pageFile(dir, entry) {
  for (const ext of [".mdx", ".md"]) {
    const f = join(dir, entry + ext);
    if (existsSync(f)) return f;
  }
  return null;
}

function collectPages(dir, order) {
  const pages = [];
  const push = (file) => {
    if (!file) return;
    const fm = readFrontmatter(file);
    pages.push({
      title: fm.title || "",
      description: fm.description || "",
      route: routeFor(file),
    });
  };
  for (const entry of order) {
    if (typeof entry !== "string" || /^---.*---$/.test(entry)) continue;
    const file = pageFile(dir, entry);
    if (file) {
      push(file);
      continue;
    }
    const subDir = join(dir, entry);
    if (existsSync(join(subDir, "meta.json")) || existsSync(join(subDir, "index.mdx"))) {
      push(pageFile(subDir, "index"));
      const subMeta = readMeta(subDir);
      for (const child of subMeta.pages || []) {
        // `index` is already pushed above as the folder's landing page.
        if (typeof child === "string" && child !== "index" && !/^---.*---$/.test(child)) {
          push(pageFile(subDir, child));
        }
      }
    }
  }
  return pages;
}

// Skip the version-pinned / internal config-reference pages; the canonical
// reference (/reference/nativelink-config) is kept.
function isNoisyRoute(route) {
  return (
    /\/reference\/nativelink-config\/v[\d.]+$/.test(route) ||
    route === "/reference/nativelink-config/main"
  );
}

function documentationSections() {
  const rootMeta = readMeta(contentRoot);
  const sections = [];
  for (const entry of rootMeta.pages || []) {
    if (typeof entry !== "string" || /^---.*---$/.test(entry) || entry === "index") {
      continue;
    }
    const dir = join(contentRoot, entry);
    if (!existsSync(join(dir, "meta.json"))) continue;
    const meta = readMeta(dir);
    const pages = collectPages(dir, meta.pages || []).filter((p) => !isNoisyRoute(p.route));
    if (pages.length) sections.push({ title: meta.title || entry, pages });
  }
  return sections;
}

// Assemble the file.
const HEADER = `# NativeLink

> High-performance, low-latency build cache and remote execution (RBE) platform — open source, written in Rust. NativeLink is an efficient implementation of the Remote Execution API (REAPI), trusted in production to handle billions of build requests per month (e.g. by Samsung), accelerating software compilation and testing while reducing infrastructure cost.

NativeLink does two things. It **caches** the results of build and test steps so unchanged work is never redone, and it **remotely executes** work across a fleet of machines so builds parallelize instead of running on one laptop. Both rely on build actions being deterministic: the same inputs always produce the same outputs, so a result can be safely reused regardless of which machine produced it.

This file is generated by scripts/gen-llms-txt.mjs — do not edit by hand.

## Supported build systems

NativeLink speaks the standard Remote Execution API, so it works with any REAPI client:

- **Bazel** — first-class; the default target for tutorials and Local Remote Execution.
- **Buck2** — remote cache and remote execution.
- **Siso** — Chromium-style remote caching and execution (Chromium is the largest public consumer).
- **Pants** — remote caching.
- **BuildStream** — artifact storage and remote execution.
- **Goma** — REAPI-compatible caching/execution.
- **CMake** — via [\`recc\`](https://buildgrid.gitlab.io/recc) as the bridge.

Runs on Unix-like systems and Windows.

## Key concepts

- **CAS (Content-Addressable Storage):** blobs are stored under their cryptographic hash (SHA-256) rather than a filename, so identical bytes collapse to a single entry (automatic de-duplication).
- **Action:** one unit of build work — a command line, input digests, platform requirements, and expected outputs. Hashing an action yields a stable identifier.
- **ActionResult:** what an action produces — output digests, exit code, stdout/stderr, timing.
- **Action Cache (AC):** a keyed store mapping \`hash(Action) → ActionResult\`. A hit skips the work entirely.
- **Digest:** a content hash plus a size; used everywhere in place of file paths.
- **Scheduler:** the dispatcher — receives \`Execute\` calls, matches actions to workers by platform properties, tracks in-flight work.
- **Worker:** the runner — fetches inputs from CAS, runs the command in a sandbox, uploads outputs to CAS.
- **Platform properties:** key/value tags on actions and on workers; the scheduler matches them.
- **Instance name:** a namespace within one cluster; action hashes from different instance names never collide.
- **Hermetic build:** outputs depend only on declared inputs, so the same inputs give the same outputs on any machine.
- **LRE (Local Remote Execution):** NativeLink on \`localhost\` with a Nix-pinned toolchain, making local builds bit-identical to remote ones.

## Crate architecture

- **\`nativelink-store\`** — all caching/storage layers and their composition.
- **\`nativelink-scheduler\`** — matches actions to worker processes and tracks execution state.
- **\`nativelink-worker\`** — local/remote sandboxed execution and telemetry.
- **\`nativelink-service\`** — the gRPC REAPI services (CAS, AC, Execution, ByteStream, Capabilities, Worker API, Health).
- **\`nativelink-config\`** — parses the JSON5 application configuration.
- **\`nativelink-proto\`** — protobuf-generated interfaces.
- **\`nativelink-util\`** — shared plumbing (hashing, retries, metrics, the \`Store\` trait).`;

async function render() {
  const { version, date } = await latestRelease();
  const stores = resolveStores();
  const sections = documentationSections();
  const examples = await listExampleNames();
  const sample = await sampleConfig();

  const out = [HEADER, ""];

  out.push(
    "## Release & license",
    "",
    `- Current release: **${version}** (${date})`,
    "- License: Functional Source License 1.1, Apache 2.0 future (`FSL-1.1-Apache-2.0`) — source-available and free for permitted self-hosted use; NativeLink Cloud and Enterprise are paid offerings.",
    "- Homepage: https://nativelink.com · Docs: https://docs.nativelink.com · Source: https://github.com/TraceMachina/nativelink",
    `- Changelog: ${DOCS_BASE}/reference/changelog`,
    "",
  );

  out.push(
    "## Stores",
    "",
    `Storage is composable: real backends and decorator stores (which wrap another store) share one \`StoreSpec\`. Each entry below is a JSON5 store key. See the [store overview](${DOCS_BASE}/reference/nativelink-config/store-overview) for which wrap which.`,
    "",
  );
  for (const s of stores) {
    out.push(`- \`${s.key}\`${s.description ? ` — ${s.description}` : ""}`);
  }
  out.push("");

  out.push(
    "## Quickstart",
    "",
    "Prebuilt Docker image (fastest; x86_64):",
    "",
    "```bash",
    `curl -O https://raw.githubusercontent.com/TraceMachina/nativelink/v${version}/nativelink-config/examples/basic_cas.json5`,
    "docker run -v $(pwd)/basic_cas.json5:/config -p 50051:50051 \\",
    `    ghcr.io/tracemachina/nativelink:v${version} config`,
    "```",
    "",
    "From source with Nix (supports macOS/WSL2; needs `nix-command`+`flakes` enabled):",
    "",
    "```bash",
    "curl -O https://raw.githubusercontent.com/TraceMachina/nativelink/main/nativelink-config/examples/basic_cas.json5",
    "nix run github:TraceMachina/nativelink ./basic_cas.json5",
    "```",
    "",
  );

  out.push(
    "## Sample configuration (`basic_cas.json5`)",
    "",
    "Inlined verbatim from `nativelink-config/examples/basic_cas.json5` (syntax-checked by tests):",
    "",
    "```json5",
    sample,
    "```",
    "",
    `More runnable examples live in [\`nativelink-config/examples/\`](${GITHUB_TREE}): ${examples
      .map((e) => `\`${e}\``)
      .join(", ")}.`,
    "",
  );

  out.push("## Documentation", "");
  for (const section of sections) {
    out.push(`### ${section.title}`);
    for (const p of section.pages) {
      const url = p.route === "/" ? DOCS_BASE : `${DOCS_BASE}${p.route}`;
      out.push(`- [${p.title}](${url})${p.description ? `: ${p.description}` : ""}`);
    }
    out.push("");
  }

  out.push(
    "## Optional",
    "",
    "- [GitHub repository](https://github.com/TraceMachina/nativelink): Source code, issues, and releases.",
    "- [Container images](https://github.com/TraceMachina/nativelink/pkgs/container/nativelink): Prebuilt Docker images (`ghcr.io/tracemachina/nativelink`).",
    `- [Config examples](${GITHUB_TREE}): Ready-to-run JSON5 configs for every backend.`,
    "- [Homepage](https://nativelink.com): Product overview, NativeLink Cloud, and Enterprise.",
    "- [Remote Execution API (REAPI)](https://github.com/bazelbuild/remote-apis): The gRPC protocol NativeLink implements.",
  );

  return `${out.join("\n")}\n`;
}

mkdirSync(dirname(outFile), { recursive: true });
writeFileSync(outFile, await render());
console.log(`gen-llms-txt: wrote ${outFile}`);
