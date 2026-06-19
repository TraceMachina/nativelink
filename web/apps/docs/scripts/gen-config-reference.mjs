#!/usr/bin/env node
// Generate the versioned NativeLink configuration reference.
//
// This is the orchestrator half of the autogeneration pipeline that was ported
// off the legacy Bazel `nativelink-config:docs_json` + `metaphase_aot.ts` setup.
// For every supported version — each `v1.x` release tag plus `main` — it:
//
//   1. materialises that version's source (the working tree for `main`, a
//      throwaway `git worktree` for each tag),
//   2. builds the schemars JSON Schema from the Rust config crate via the
//      `build-schema` binary (`cargo run --bin build-schema --features
//      dev-schema`; the same artifact `bazel run //nativelink-config:build-schema`
//      produces),
//   3. runs the pure transform in `./lib/schema-to-mdx.mjs`, and
//   4. writes a versioned MDX page.
//
// It also emits `lib/config-versions.ts` (the manifest the `<ConfigVersionSwitcher>`
// reads) and the folder `meta.json`.
//
// Usage from web/:
//   bun --filter @nativelink/docs gen:config-reference            # all versions
//   bun --filter @nativelink/docs gen:config-reference main       # just main
//   bun --filter @nativelink/docs gen:config-reference v1.5.1     # release
//
// Requires a Rust toolchain (cargo). Tag builds use a shared CARGO_TARGET_DIR so
// the worktrees don't each spend disk on a full target/ tree.

import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { schemaToMdx } from "./lib/schema-to-mdx.mjs";

const SCHEMA_FILE = "nativelink_config.schema.json";
const GITHUB_BASE = "https://github.com/TraceMachina/nativelink";
const REFERENCE_BASE = "/reference/nativelink-config";

const here = dirname(fileURLToPath(import.meta.url));
const docsAppDir = join(here, ".."); // web/apps/docs
const contentDir = join(docsAppDir, "content/docs/reference/nativelink-config");
const manifestFile = join(docsAppDir, "lib/config-versions.ts");

function git(args, opts = {}) {
  return execFileSync("git", args, {
    encoding: "utf8",
    cwd: opts.cwd ?? repoRoot,
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

function errorSummary(err) {
  const parts = [String(err.message || err).split("\n")[0]];
  const stderr = err.stderr?.toString?.().trim();
  if (stderr) parts.push(stderr.split("\n")[0]);
  return parts.join(": ");
}

// Resolve the nativelink repo root from this script's location.
const repoRoot = execFileSync("git", ["rev-parse", "--show-toplevel"], {
  encoding: "utf8",
  cwd: here,
}).trim();

/** Parse `vMAJOR.MINOR.PATCH` (no pre-release) → [maj, min, patch] or null. */
function parseRelease(tag) {
  const m = /^v(\d+)\.(\d+)\.(\d+)$/.exec(tag);
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

const cmpRelease = (a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2];

/** Build the ordered version list: main (dev) + every v1.x release, newest
 *  first, with the highest release flagged as the canonical "latest". */
function resolveVersions() {
  const releaseTags = git(["tag", "--list", "v1.*"])
    .split("\n")
    .map((t) => t.trim())
    .filter(Boolean)
    .map((t) => ({ tag: t, sv: parseRelease(t) }))
    .filter((t) => t.sv) // drop rc/pre-release tags
    .sort((a, b) => cmpRelease(b.sv, a.sv));

  if (releaseTags.length === 0) {
    throw new Error("No v1.x release tags found in the nativelink repo.");
  }
  const latest = releaseTags[0].tag;

  /** @type {Array<{version:string,label:string,ref:string,slug:string|null,isLatest:boolean,isDev:boolean}>} */
  const versions = [
    {
      version: "main",
      label: "main (experimental)",
      ref: "HEAD",
      slug: "main",
      isLatest: false,
      isDev: true,
    },
    ...releaseTags.map(({ tag }) => ({
      version: tag,
      label: tag === latest ? `${tag} (latest)` : tag,
      ref: tag,
      slug: tag === latest ? null : tag, // latest === the folder index
      isLatest: tag === latest,
      isDev: false,
    })),
  ];

  return versions;
}

function selectVersions(versions, filter) {
  if (!filter || filter.length === 0) {
    return versions;
  }

  const byVersion = new Map(versions.map((v) => [v.version, v]));
  const unknown = filter.filter((version) => !byVersion.has(version));
  if (unknown.length) {
    throw new Error(`Unknown config reference version(s): ${unknown.join(", ")}`);
  }

  const selected = new Map();
  for (const version of filter) {
    selected.set(version, byVersion.get(version));
  }

  const latest = versions.find((v) => v.isLatest);
  if (latest && selected.has(latest.version)) {
    const previousLatest = versions.find((v) => !v.isDev && !v.isLatest);
    if (previousLatest) {
      selected.set(previousLatest.version, previousLatest);
    }
  }

  return versions.filter((v) => selected.has(v.version));
}

function shouldWriteManifest(versions, filter) {
  if (!filter || filter.length === 0) {
    return true;
  }
  return versions.some((v) => v.isLatest && filter.includes(v.version));
}

function hrefFor(v) {
  return v.slug === null ? REFERENCE_BASE : `${REFERENCE_BASE}/${v.slug}`;
}

function outFileFor(v) {
  const name = v.slug === null ? "index" : v.slug;
  return join(contentDir, `${name}.mdx`);
}

/** Run `build-schema` for a given source tree and return the parsed schema. */
function buildSchema(sourceDir, sharedTargetDir) {
  const env = { ...process.env };
  if (sharedTargetDir) env.CARGO_TARGET_DIR = sharedTargetDir;
  execFileSync(
    "cargo",
    [
      "run",
      "--quiet",
      "--bin",
      "build-schema",
      "--features",
      "dev-schema",
      "--package",
      "nativelink-config",
    ],
    { cwd: sourceDir, env, stdio: ["ignore", "inherit", "inherit"] },
  );
  const schemaPath = join(sourceDir, SCHEMA_FILE);
  const schema = JSON.parse(readFileSync(schemaPath, "utf8"));
  rmSync(schemaPath, { force: true }); // don't leave it in the tree
  return schema;
}

function generateOne(v, sharedTargetDir) {
  const commit = git(["rev-parse", "--short", v.ref]);
  // GitHub source links should point at a real ref, not the local "HEAD".
  const sourceRef = v.isDev ? "main" : v.ref;

  let sourceDir;
  let worktree;
  if (v.isDev) {
    sourceDir = repoRoot; // generate main from the live working tree
  } else {
    const worktreeLabel = v.slug ?? v.version;
    worktree = mkdtempSync(join(tmpdir(), `nl-cfg-${worktreeLabel}-`));
    rmSync(worktree, { recursive: true, force: true });
    git(["worktree", "add", "--quiet", "--detach", worktree, v.ref]);
    sourceDir = worktree;
  }

  try {
    const schema = buildSchema(sourceDir, v.isDev ? null : sharedTargetDir);
    const mdx = schemaToMdx(schema, {
      version: v.version,
      ref: sourceRef,
      commit,
      isLatest: v.isLatest,
      switcher: "<ConfigVersionSwitcher />",
      githubBase: GITHUB_BASE,
    });
    writeFileSync(outFileFor(v), mdx);
    return { ...v, commit, defs: Object.keys(schema.$defs ?? schema.definitions ?? {}).length };
  } finally {
    if (worktree) {
      try {
        git(["worktree", "remove", "--force", worktree]);
      } catch {
        // If `git worktree add` failed before registration, normal rm cleanup
        // below is enough.
      }
      rmSync(worktree, { recursive: true, force: true });
    }
  }
}

function writeManifest(versions) {
  const entries = versions.map((v) => ({
    version: v.version,
    label: v.label,
    href: hrefFor(v),
    ref: v.isDev ? "main" : v.ref,
    isLatest: v.isLatest,
    isDev: v.isDev,
  }));
  const body = `// AUTOGENERATED by scripts/gen-config-reference.mjs — do not edit by hand.
// Regenerate from web/: bun --filter @nativelink/docs gen:config-reference

export interface ConfigVersion {
  /** Display version, e.g. "v1.5.0" or "main". */
  version: string;
  /** Human label shown in the switcher, e.g. "v1.5.0 (latest)". */
  label: string;
  /** Route for this version's reference page. */
  href: string;
  /** Git ref the page was generated from. */
  ref: string;
  /** Whether this is the canonical (latest stable) page. */
  isLatest: boolean;
  /** Whether this is the in-development ("main") page. */
  isDev: boolean;
}

export const CONFIG_REFERENCE_BASE = ${JSON.stringify(REFERENCE_BASE)};

export const CONFIG_VERSIONS: ConfigVersion[] = ${JSON.stringify(entries, null, 2)};
`;
  writeFileSync(manifestFile, body);
}

function writeMeta() {
  // Only the canonical page appears in the sidebar; other versions are reached
  // through the in-page version switcher.
  writeFileSync(
    join(contentDir, "meta.json"),
    `${JSON.stringify({ pages: ["index"], title: "NativeLink config" }, null, 2)}\n`,
  );
}

function main() {
  const filter = process.argv.slice(2);
  const allVersions = resolveVersions();
  const versions = selectVersions(allVersions, filter);
  const updateManifest = shouldWriteManifest(allVersions, filter);

  mkdirSync(contentDir, { recursive: true });
  git(["worktree", "prune"]);

  // Remove the legacy single-file page so the folder route can take over.
  rmSync(join(dirname(contentDir), "nativelink-config.mdx"), { force: true });

  const sharedTargetDir = mkdtempSync(join(tmpdir(), "nl-cfg-target-"));
  const generated = [];
  const skipped = [];

  for (const v of versions) {
    process.stdout.write(`\n▸ ${v.version} (${v.ref}) … `);
    try {
      const result = generateOne(v, sharedTargetDir);
      generated.push(result);
      process.stdout.write(`ok — ${result.defs} types\n`);
    } catch (err) {
      const summary = errorSummary(err);
      skipped.push({ version: v.version, error: summary });
      process.stdout.write(`SKIPPED\n  ${summary}\n`);
    }
  }

  rmSync(sharedTargetDir, { recursive: true, force: true });

  if (skipped.length) {
    console.log(`\nSkipped ${skipped.length}:`);
    for (const s of skipped) console.log(`  ${s.version}: ${s.error}`);
    throw new Error(
      `Refusing to write a partial config reference manifest; skipped ${skipped.length} version(s).`,
    );
  }

  if (generated.length === 0) {
    throw new Error("No versions generated; refusing to write an empty manifest.");
  }

  if (updateManifest) {
    writeManifest(allVersions);
    writeMeta();
  }

  console.log(`\nGenerated ${generated.length} version(s):`);
  for (const v of generated) {
    console.log(`  ${v.label.padEnd(20)} ${hrefFor(v)}`);
  }
  if (updateManifest) {
    console.log(`\nUpdated manifest with ${allVersions.length} version(s).`);
  }
}

main();
