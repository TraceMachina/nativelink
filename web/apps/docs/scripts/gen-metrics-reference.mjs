#!/usr/bin/env node
// Generate the NativeLink metrics reference from the Rust source.
//
// Unlike `gen-config-reference.mjs`, this generator needs no Rust toolchain: the
// metric declarations live in one file (`nativelink-util/src/metrics.rs`) as
// plain `meter.<kind>("name")` builder chains, and the question this page exists
// to answer (*is this metric actually emitted?*) is answered by finding call
// sites, which is a source scan either way. Running it costs a few hundred
// milliseconds and no build.
//
// Usage, from web/:
//   bun --filter @nativelink/docs gen:metrics-reference
// or directly:
//   node scripts/gen-metrics-reference.mjs
//
// What it does:
//   1. Parses every instrument out of the `LazyLock` metric groups in metrics.rs:
//      field name, OTLP name, Rust instrument type, unit, description, and
//      histogram bucket boundaries.
//   2. Parses the attribute-key constants and the `impl From<Enum> for Value`
//      blocks that define the closed set of values each attribute can take.
//   3. Scans every crate for `.field.add(..)` / `.field.record(..)` call sites,
//      and for indirect references (a field handed to a helper), resolving the
//      helper back to its own call sites. Test files do not count.
//   4. Scans the shipped observability artifacts for `nativelink_*` series names
//      and matches them back to instruments, so the page can report both
//      directions of drift: instruments nothing emits, and series nothing
//      declares.
//   5. Feeds the inventory to the pure `metricsToMdx` transform.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { metricsToMdx } from "./lib/metrics-to-mdx.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const docsRoot = join(here, "..");
// The checkout to scan. Defaults to the repository this docs app lives in;
// override to document a different checkout, for example a newer main than the
// docs branch is based on.
const repoRoot = process.env.NATIVELINK_REPO_ROOT ?? join(docsRoot, "..", "..", "..");

const METRICS_RS = "nativelink-util/src/metrics.rs";
const GITHUB_BASE = "https://github.com/TraceMachina/nativelink";
const OUT_FILE = join(docsRoot, "content/docs/reference/metrics.mdx");

/** Crates scanned for metric call sites. */
const SOURCE_DIRS = [
  "nativelink-store",
  "nativelink-scheduler",
  "nativelink-service",
  "nativelink-worker",
  "nativelink-util",
  "nativelink-config",
  "src",
];

/** Shipped observability artifacts scanned for `nativelink_*` series names. */
const ARTIFACT_DIRS = ["deployment-examples/metrics", "kubernetes"];
const ARTIFACT_EXTS = [".yaml", ".yml", ".json", ".md"];

/** Which crate owns a call site, and what that implies about when it runs. */
const GATES = [
  {
    file: "nativelink-store/src/cache_metrics_store.rs",
    verdict: "gated",
    gate: "only for stores explicitly wrapped in a `cache_metrics` spec",
  },
  {
    prefix: "nativelink-scheduler/",
    verdict: "gated",
    gate: "scheduler process only",
  },
  {
    file: "nativelink-store/src/fast_slow_store.rs",
    verdict: "gated",
    gate: "only through a `fast_slow` store",
  },
];

// --------------------------------------------------------------------------
// Small helpers.
// --------------------------------------------------------------------------

function git(args) {
  return execFileSync("git", args, { cwd: repoRoot, encoding: "utf8" }).trim();
}

function lineOf(text, index) {
  let line = 1;
  for (let i = 0; i < index; i++) if (text.charCodeAt(i) === 10) line++;
  return line;
}

/** Index just past the `{` … `}` block that starts at `open`. */
function matchBrace(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    const c = text[i];
    if (c === "{") depth++;
    else if (c === "}") {
      depth--;
      if (depth === 0) return i;
    }
  }
  return text.length;
}

function walk(dir, exts, out = []) {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return out;
  }
  for (const name of entries) {
    if (name === "target" || name === "node_modules" || name === ".git") continue;
    const full = join(dir, name);
    let st;
    try {
      st = statSync(full);
    } catch {
      continue;
    }
    if (st.isDirectory()) walk(full, exts, out);
    else if (exts.some((e) => name.endsWith(e))) out.push(full);
  }
  return out;
}

/** Strip Rust `///` and `//!` doc-comment markers from a captured run. */
function docText(raw) {
  return raw
    .split("\n")
    .map((l) => l.trim().replace(/^\/{3}!?/, "").replace(/^\/{2}!/, "").trim())
    .join("\n")
    .trim();
}

/** The `///` block immediately preceding `index`, if any. */
function precedingDoc(text, index) {
  const before = text.slice(0, index);
  const m = before.match(/((?:[ \t]*\/\/\/[^\n]*\n)+)[ \t]*(?:#\[[^\]]*\]\s*)*$/);
  return m ? docText(m[1]) : "";
}

// --------------------------------------------------------------------------
// 1. Instrument declarations.
// --------------------------------------------------------------------------

const RUST_KIND = {
  f64_histogram: { kind: "Histogram", rust: "Histogram<f64>" },
  u64_histogram: { kind: "Histogram", rust: "Histogram<u64>" },
  u64_counter: { kind: "Counter", rust: "Counter<u64>" },
  f64_counter: { kind: "Counter", rust: "Counter<f64>" },
  i64_up_down_counter: { kind: "UpDownCounter", rust: "UpDownCounter<i64>" },
  f64_up_down_counter: { kind: "UpDownCounter", rust: "UpDownCounter<f64>" },
  u64_gauge: { kind: "Gauge", rust: "Gauge<u64>" },
  i64_gauge: { kind: "Gauge", rust: "Gauge<i64>" },
  f64_gauge: { kind: "Gauge", rust: "Gauge<f64>" },
};

/** Concatenated Rust string literal(s) inside a call, e.g. `.with_description("a" "b")`. */
function stringArg(chunk) {
  const parts = [...chunk.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) =>
    m[1].replace(/\\"/g, '"').replace(/\\n/g, "\n").replace(/\\\\/g, "\\"),
  );
  return parts.join("");
}

function parseGroups(src) {
  const groups = [];
  const groupRe = /pub static (\w+): LazyLock<(\w+)> = LazyLock::new\(\|\| \{/g;
  let g = groupRe.exec(src);
  while (g !== null) {
    const [, name, struct] = g;
    const open = src.indexOf("{", g.index + g[0].length - 1);
    const end = matchBrace(src, open);
    const block = src.slice(open, end);
    const blockOffset = open;

    // Field docs live on the struct, not the builder.
    const structRe = new RegExp(`pub struct ${struct} \\{`);
    const sm = structRe.exec(src);
    const fieldDocs = new Map();
    let structRange = null;
    if (sm) {
      const sOpen = src.indexOf("{", sm.index);
      const sEnd = matchBrace(src, sOpen);
      structRange = [sOpen, sEnd];
      const sBlock = src.slice(sOpen, sEnd);
      const fRe = /((?:[ \t]*\/\/\/[^\n]*\n)+)[ \t]*pub (\w+):/g;
      let f = fRe.exec(sBlock);
      while (f !== null) {
        fieldDocs.set(f[2], docText(f[1]));
        f = fRe.exec(sBlock);
      }
    }

    const metrics = [];
    const instRe = /(\w+): meter\s*\.(\w+)\("([^"]+)"\)/g;
    let i = instRe.exec(block);
    while (i !== null) {
      const [, field, builder, otelName] = i;
      const kindInfo = RUST_KIND[builder];
      if (!kindInfo) {
        throw new Error(
          `Unknown OpenTelemetry builder \`${builder}\` for \`${field}\`; teach RUST_KIND about it.`,
        );
      }
      // The chain runs to the next `.build()`.
      const chainEnd = block.indexOf(".build()", i.index);
      const chain = block.slice(i.index, chainEnd === -1 ? block.length : chainEnd);

      const descM = /\.with_description\(([\s\S]*?)\)\s*\n/.exec(chain);
      const unitM = /\.with_unit\("([^"]*)"\)/.exec(chain);
      const boundsM = /\.with_boundaries\(vec!\[([\s\S]*?)\]\)/.exec(chain);
      const boundaries = boundsM
        ? boundsM[1]
            .replace(/\/\/[^\n]*/g, "")
            .split(",")
            .map((s) => s.trim())
            .filter(Boolean)
        : [];

      metrics.push({
        field,
        otelName,
        kind: kindInfo.kind,
        rustType: kindInfo.rust,
        unit: unitM ? unitM[1] : "",
        description: fieldDocs.get(field) || (descM ? stringArg(descM[1]) : ""),
        boundaries,
        declaredAt: { file: METRICS_RS, line: lineOf(src, blockOffset + i.index) },
      });
      i = instRe.exec(block);
    }

    groups.push({
      name,
      struct,
      doc: precedingDoc(src, g.index),
      declaredAt: { file: METRICS_RS, line: lineOf(src, g.index) },
      excluded: [[open, end], structRange].filter(Boolean),
      metrics,
    });
    g = groupRe.exec(src);
  }
  return groups;
}

// --------------------------------------------------------------------------
// 2. Attribute keys and their closed value sets.
// --------------------------------------------------------------------------

function parseAttributes(src) {
  const keys = [];
  const re = /((?:[ \t]*\/\/[^\n/][^\n]*\n)*)pub const (\w+): &str = "([^"]+)";/g;
  let m = re.exec(src);
  while (m !== null) {
    keys.push({
      const: m[2],
      key: m[3],
      comment: docText(m[1]),
      line: lineOf(src, m.index),
    });
    m = re.exec(src);
  }

  const valueEnums = [];
  const implRe = /impl From<(\w+)> for Value \{([\s\S]*?)\n\}/g;
  let im = implRe.exec(src);
  while (im !== null) {
    const enumName = im[1];
    const arms = [...im[2].matchAll(/(\w+)::(\w+) => Self::from\("([^"]+)"\)/g)].map((a) => ({
      variant: a[2],
      value: a[3],
    }));
    if (arms.length) {
      // Pull each variant's `///` doc off the enum declaration.
      const decl = new RegExp(`pub enum ${enumName} \\{`).exec(src);
      const docs = new Map();
      if (decl) {
        const open = src.indexOf("{", decl.index);
        const body = src.slice(open, matchBrace(src, open));
        const vRe = /((?:[ \t]*\/\/\/[^\n]*\n)+)[ \t]*(\w+),/g;
        let v = vRe.exec(body);
        while (v !== null) {
          docs.set(v[2], docText(v[1]));
          v = vRe.exec(body);
        }
      }
      valueEnums.push({
        name: enumName,
        variants: arms.map((a) => ({ ...a, doc: docs.get(a.variant) || "" })),
      });
    }
    im = implRe.exec(src);
  }
  return { keys, valueEnums };
}

/** Map an attribute-key constant to the enum whose values fill it, by name. */
const ATTR_ENUM = {
  CACHE_OPERATION: "CacheOperationName",
  CACHE_RESULT: "CacheOperationResult",
  EXECUTION_STAGE: "ExecutionStage",
  EXECUTION_RESULT: "ExecutionResult",
};

// --------------------------------------------------------------------------
// 3. Call sites.
// --------------------------------------------------------------------------

function isTestPath(rel) {
  return /(^|\/)tests?\//.test(rel) || rel.endsWith("_test.rs") || rel.endsWith("_tests.rs");
}

function sourceFiles() {
  const files = [];
  for (const d of SOURCE_DIRS) files.push(...walk(join(repoRoot, d), [".rs"]));
  return files
    .map((f) => ({ abs: f, rel: relative(repoRoot, f).split("\\").join("/") }))
    .filter((f) => !isTestPath(f.rel));
}

/** Nearest enclosing `fn NAME(` before `index`. */
function enclosingFn(text, index) {
  const before = text.slice(0, index);
  const matches = [...before.matchAll(/\bfn\s+(\w+)\s*\(/g)];
  return matches.length ? matches[matches.length - 1][1] : null;
}

function findCallSites(files, metricsSrc, groups) {
  /** field -> { direct: [site], indirect: [site], viaFn: Set<string> } */
  const byField = new Map();
  const fields = [];
  for (const g of groups) for (const m of g.metrics) fields.push(m.field);
  for (const f of fields) byField.set(f, { direct: [], indirect: [], viaFn: new Set() });

  const excludedInMetricsRs = groups.flatMap((g) => g.excluded);
  const inExcluded = (idx) => excludedInMetricsRs.some(([a, b]) => idx >= a && idx <= b);

  for (const { abs, rel } of files) {
    const text = readFileSync(abs, "utf8");
    for (const field of fields) {
      const anyRe = new RegExp(`\\.\\s*${field}\\b`, "g");
      let m = anyRe.exec(text);
      while (m !== null) {
        const isMetricsRs = rel === METRICS_RS;
        if (isMetricsRs && inExcluded(m.index)) {
          m = anyRe.exec(text);
          continue;
        }
        const after = text.slice(m.index + m[0].length, m.index + m[0].length + 40);
        const direct = /^\s*\.\s*(add|record)\s*\(/.test(after);
        const site = { file: rel, line: lineOf(text, m.index) };
        const entry = byField.get(field);
        if (direct) entry.direct.push(site);
        else entry.indirect.push(site);
        // Any reference inside metrics.rs itself sits in a helper body; credit
        // it to whoever calls that helper, not to metrics.rs.
        if (!direct || isMetricsRs) {
          const fn = enclosingFn(text, m.index);
          if (fn) entry.viaFn.add(`${rel}::${fn}`);
        }
        m = anyRe.exec(text);
      }
    }
  }

  // An indirect reference inside metrics.rs only emits if the helper that holds
  // it is itself called from somewhere that is not metrics.rs and not a test.
  const helperCallers = new Map();
  const helpers = new Set();
  for (const entry of byField.values())
    for (const v of entry.viaFn) if (v.startsWith(`${METRICS_RS}::`)) helpers.add(v.split("::")[1]);

  for (const helper of helpers) {
    const callers = [];
    const re = new RegExp(`\\b${helper}\\s*\\(`, "g");
    for (const { abs, rel } of files) {
      if (rel === METRICS_RS) continue;
      const text = readFileSync(abs, "utf8");
      let m = re.exec(text);
      while (m !== null) {
        callers.push({ file: rel, line: lineOf(text, m.index) });
        m = re.exec(text);
      }
      re.lastIndex = 0;
    }
    helperCallers.set(helper, callers);
  }

  return { byField, helperCallers };
}

function verdictFor(field, { byField, helperCallers }) {
  const entry = byField.get(field);
  const sites = [];
  for (const s of entry.direct) if (s.file !== METRICS_RS) sites.push(s);

  // Direct calls inside metrics.rs (outside the declaration blocks) are helper
  // bodies; credit them to the helper's own callers.
  const helperSites = [];
  for (const via of entry.viaFn) {
    const [file, fn] = via.split("::");
    if (file !== METRICS_RS) continue;
    for (const c of helperCallers.get(fn) ?? []) helperSites.push({ ...c, via: fn });
  }
  // Deduplicate: two instruments recorded by the same helper share its callers.
  const all = [...sites, ...helperSites].filter(
    (s, i, a) => a.findIndex((o) => o.file === s.file && o.line === s.line) === i,
  );

  if (all.length === 0) {
    return { verdict: "never", gate: "no call site anywhere in the workspace", sites: [] };
  }

  const gate = GATES.find((g) =>
    all.every((s) => (g.file ? s.file === g.file : s.file.startsWith(g.prefix))),
  );
  if (gate) return { verdict: gate.verdict, gate: gate.gate, sites: all };
  return { verdict: "emitted", gate: "whenever the code path runs", sites: all };
}

// --------------------------------------------------------------------------
// 4. Shipped observability artifacts.
// --------------------------------------------------------------------------

function scanArtifacts() {
  const found = new Map(); // series name -> [{file, line}]
  for (const d of ARTIFACT_DIRS) {
    for (const abs of walk(join(repoRoot, d), ARTIFACT_EXTS)) {
      const rel = relative(repoRoot, abs).split("\\").join("/");
      const text = readFileSync(abs, "utf8");
      const re = /\bnativelink_[a-z0-9_]+/g;
      let m = re.exec(text);
      while (m !== null) {
        const name = m[0];
        // `- name: nativelink_performance` is a Prometheus *rule group*, not a
        // series. Same for an alert-rule group. Skip those or every group name
        // reads as an orphan series.
        const lineStart = text.lastIndexOf("\n", m.index) + 1;
        if (/^\s*-?\s*name:\s*$/.test(text.slice(lineStart, m.index))) {
          m = re.exec(text);
          continue;
        }
        if (!found.has(name)) found.set(name, []);
        const sites = found.get(name);
        if (sites.length < 4) sites.push({ file: rel, line: lineOf(text, m.index) });
        m = re.exec(text);
      }
    }
  }
  return found;
}

const SUFFIXES = ["_bucket", "_sum", "_count", "_total"];

/** Base series name an instrument would produce, before exporter suffixes. */
function promBase(otelName) {
  return `nativelink_${otelName.replace(/\./g, "_")}`;
}

function stripSuffix(name) {
  for (const s of SUFFIXES) if (name.endsWith(s)) return name.slice(0, -s.length);
  return name;
}

// --------------------------------------------------------------------------
// Assemble.
// --------------------------------------------------------------------------

const GROUP_TITLES = {
  CACHE_METRICS: "Cache metrics",
  EXECUTION_METRICS: "Execution metrics",
  WORKER_METRICS: "Worker fleet metrics",
  RPC_METRICS: "gRPC server metrics",
  SCHEDULER_METRICS: "Scheduler metrics",
  STORE_TIER_METRICS: "Store tier metrics",
  HEALTH_METRICS: "Health check metrics",
  CONNECTION_METRICS: "Connection pool metrics",
};

const GROUP_GATES = {
  CACHE_METRICS:
    "Every cache instrument is emitted from one place: the `cache_metrics` store wrapper. A store that is not wrapped emits nothing, no matter how the collector is configured.",
  EXECUTION_METRICS:
    "Execution instruments are emitted from the scheduler process as actions change state, plus the per-action CPU time and peak memory a worker reports back when it finishes. Worker processes emit no execution metrics of their own.",
  WORKER_METRICS:
    "Worker fleet instruments are emitted from the scheduler process as workers connect, heartbeat, pause, drain and leave. They describe the pool as this scheduler instance sees it.",
  RPC_METRICS:
    "Emitted by the `OtlpLayer` tower middleware on every gRPC server NativeLink starts, so a CAS, a scheduler and a worker API endpoint each report their own.",
  SCHEDULER_METRICS:
    "Emitted from the scheduler process once per matching pass, whether or not the pass assigned anything.",
  STORE_TIER_METRICS:
    "Emitted from the `fast_slow` store wrapper only. A deployment without a `fast_slow` store emits none of these.",
  HEALTH_METRICS:
    "Emitted each time the health check endpoint runs, one observation per registered health indicator.",
  CONNECTION_METRICS:
    "Emitted by the gRPC connection manager and the Redis store as they hand out and re-establish connections. A deployment that uses neither emits none of these.",
};

function main() {
  const metricsPath = join(repoRoot, METRICS_RS);
  if (!existsSync(metricsPath)) {
    throw new Error(`Cannot find ${METRICS_RS}. Is this running inside the NativeLink repo?`);
  }
  const src = readFileSync(metricsPath, "utf8");

  let ref = "main";
  let commit = "";
  try {
    commit = git(["rev-parse", "--short", "HEAD"]);
    ref = git(["describe", "--tags", "--abbrev=0"]);
  } catch {
    // Detached or shallow checkout; fall back to `main`.
  }

  const groups = parseGroups(src);
  if (groups.length === 0) throw new Error("Parsed zero metric groups; the parser has drifted.");

  const files = sourceFiles();
  const calls = findCallSites(files, src, groups);
  const artifacts = scanArtifacts();

  const matchedArtifactNames = new Set();
  for (const g of groups) {
    g.title = GROUP_TITLES[g.name] ?? g.name;
    g.gate = GROUP_GATES[g.name] ?? "";
    for (const m of g.metrics) {
      m.emit = verdictFor(m.field, calls);
      m.promNameGuess = promBase(m.otelName);
      const base = m.promNameGuess;
      const hits = [...artifacts.entries()].filter(([name]) => stripSuffix(name) === base);
      if (hits.length) {
        // Prefer the un-suffixed form when both appear.
        hits.sort((a, b) => a[0].length - b[0].length);
        m.promName = hits[0][0];
        m.artifacts = hits.flatMap(([, sites]) => sites).slice(0, 4);
        for (const [name] of hits) matchedArtifactNames.add(name);
      } else {
        m.promName = null;
        m.artifacts = [];
      }
    }
  }

  const { keys, valueEnums } = parseAttributes(src);
  for (const e of valueEnums) {
    const attr = Object.entries(ATTR_ENUM).find(([, v]) => v === e.name);
    if (attr) e.attribute = keys.find((k) => k.const === attr[0])?.key ?? e.name;
  }
  for (const k of keys) {
    const enumName = ATTR_ENUM[k.const];
    k.values = enumName ? (valueEnums.find((e) => e.name === enumName)?.variants ?? []) : [];
  }

  const attributeGroups = [
    { title: "Cache attributes", keys: keys.filter((k) => k.key.startsWith("cache.")) },
    { title: "Execution attributes", keys: keys.filter((k) => k.key.startsWith("execution.")) },
    { title: "Worker attributes", keys: keys.filter((k) => k.key.startsWith("worker.")) },
    { title: "gRPC attributes", keys: keys.filter((k) => k.key.startsWith("rpc.")) },
    { title: "Scheduler attributes", keys: keys.filter((k) => k.key.startsWith("scheduler.")) },
    { title: "Store tier attributes", keys: keys.filter((k) => k.key.startsWith("store.")) },
    { title: "Health attributes", keys: keys.filter((k) => k.key.startsWith("health.")) },
    { title: "Connection attributes", keys: keys.filter((k) => k.key.startsWith("connection.")) },
  ].filter((g) => g.keys.length);

  // Series the shipped config mentions that no instrument produces. Names that
  // are recording-rule outputs (`nativelink:`-prefixed in Prometheus, but written
  // with an underscore in a few places) and the rule-group names themselves are
  // not series, so drop anything that matches no instrument *and* is a prefix of
  // no instrument base.
  const allBases = new Set(groups.flatMap((g) => g.metrics.map((m) => m.promNameGuess)));
  const orphanArtifactNames = [...artifacts.entries()]
    .filter(([name]) => !matchedArtifactNames.has(name))
    .filter(([name]) => {
      const base = stripSuffix(name);
      if (allBases.has(base)) return false;
      // Rule-group names (`nativelink_execution`, `nativelink_cache`, …) and
      // truncated fragments are prefixes of a real base; they are not series.
      for (const b of allBases) if (b.startsWith(name) || b.startsWith(base)) return false;
      return true;
    })
    .map(([name, sites]) => ({ name, sites }));

  const inventory = { groups, attributeGroups, valueEnums, orphanArtifactNames };
  const mdx = metricsToMdx(inventory, {
    version: ref,
    ref,
    commit,
    githubBase: GITHUB_BASE,
  });
  writeFileSync(OUT_FILE, mdx);

  const total = groups.reduce((n, g) => n + g.metrics.length, 0);
  const never = groups.reduce(
    (n, g) => n + g.metrics.filter((m) => m.emit.verdict === "never").length,
    0,
  );
  console.log(`Wrote ${relative(repoRoot, OUT_FILE)}`);
  console.log(`  ${total} instruments in ${groups.length} groups (${ref}${commit ? ` ${commit}` : ""})`);
  console.log(`  ${never} never emitted, ${orphanArtifactNames.length} orphan series in shipped config`);
  for (const g of groups) {
    for (const m of g.metrics) {
      const flag = m.emit.verdict === "never" ? "NEVER  " : m.emit.verdict === "gated" ? "gated  " : "emitted";
      console.log(`  ${flag} ${m.otelName}`);
    }
  }
}

main();
