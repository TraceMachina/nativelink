#!/usr/bin/env node
// Anti-drift lint for the configuration snippets embedded in the docs.
//
// The configuration reference cannot drift from the binary; it is generated
// from the same Rust types the binary deserializes. Prose pages are a different
// matter: a how-to that shows `evict_bytes` keeps showing `evict_bytes` long
// after the field is renamed, and nothing fails. Readers copy the snippet, the
// binary rejects it with `unknown field`, and the docs look like a liar.
//
// This lint closes that gap without needing a Rust toolchain. It reads the
// *generated* configuration reference (the one that provably matches the
// binary), harvests every field name and every tagged-variant key out of it,
// and then checks that every key used in every JSON5 snippet under
// `content/docs/` appears there.
//
// Usage, from web/:
//   bun --filter @nativelink/docs lint:snippets
// or directly:
//   node scripts/lint-snippets.mjs
//
// Escapes, for the snippets that legitimately are not NativeLink config:
//   - Put `{/* lint-snippets: ignore */}` on the line before the fence.
//   - Keys nested under a free-form map (`properties`, `platform_properties`,
//     `env`, …) are skipped automatically; those are operator-chosen names.
//
// Exit code is 1 if any snippet uses a key the reference does not know.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const docsRoot = join(here, "..");
const contentDir = join(docsRoot, "content/docs");
const REFERENCE = join(contentDir, "reference/nativelink-config/index.mdx");

/** Keys under these parents are operator-chosen, not config fields. */
const FREEFORM_PARENTS = new Set([
  "properties",
  "platform_properties",
  "supported_platform_properties",
  "property_modifications",
  "env",
  "environment",
  "labels",
  "annotations",
  "const_labels",
  "additional_environment",
  "upload_action_result",
  "headers",
]);

/**
 * Keys that are real config but cannot be harvested from the reference tables,
 * because they are map *keys* rather than struct fields. Keep this list short
 * and justified; every entry is a hole in the lint.
 */
const KNOWN_MAP_KEYS = new Set([
  // `services.experimental_*` and store names are user-chosen; the entries below
  // are the fixed map keys the schema documents in prose rather than as fields.
  "main", // conventional instance_name in every example

  // Real fields the generated reference does not mention anywhere, because
  // `ExperimentalCloudObjectSpec` renders as a stub; the provider-specific
  // halves of that spec never reach a props table or an example. Every entry
  // here is a hole in the lint AND a gap in the reference; the fix is to teach
  // `scripts/lib/schema-to-mdx.mjs` to expand that spec, after which these
  // should be deleted.
  "sas_url", // nativelink-config/src/stores.rs:1233
]);

const IGNORE_MARKER = "lint-snippets: ignore";

// --------------------------------------------------------------------------

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    const st = statSync(full);
    if (st.isDirectory()) walk(full, out);
    else if (name.endsWith(".mdx") || name.endsWith(".md")) out.push(full);
  }
  return out;
}

/** Every field name and tagged-variant key the generated reference documents. */
function harvestKnownKeys(referencePath) {
  const text = readFileSync(referencePath, "utf8");
  const known = new Set(KNOWN_MAP_KEYS);

  // Props-table rows: `| \`field\` | type | … |`
  for (const m of text.matchAll(/^\|\s*`([a-z0-9_]+)`\s*\|/gm)) known.add(m[1]);
  // Tagged-enum variant headings: "### \`fast_slow\`"
  for (const m of text.matchAll(/^###\s+`([a-z0-9_]+)`\s*$/gm)) known.add(m[1]);
  // Keys used in the reference's own JSON5 examples. Those examples come from
  // the doc comments on the same Rust structs the tables come from, so they are
  // as authoritative as the tables, and they are the only place some specs get
  // documented at all (see the stub harvest below).
  for (const block of text.matchAll(/^\s*```json5?\n([\s\S]*?)^\s*```\s*$/gm)) {
    for (const m of block[1].matchAll(/^\s*"([a-z0-9_]+)"\s*:/gm)) known.add(m[1]);
  }
  // Some specs are rendered as a stub ("## ExperimentalCloudObjectSpec" whose
  // whole body is "See [`experimental_cloud_object_store`](#…) for details")
  // because the schema transform cannot flatten them into a props table. Their
  // fields exist only as inline code in the narrative section the stub points
  // at, so harvest that section. Scoped deliberately: harvesting inline code
  // from the whole reference would make this lint pass on anything.
  for (const stub of text.matchAll(/^##\s+\w+\s*\n+See \[`([a-z0-9_]+)`\]\([^)]*\) for details/gm)) {
    const start = text.search(new RegExp(`^###\\s+\`${stub[1]}\`\\s*$`, "m"));
    if (start === -1) continue;
    const rest = text.slice(start + 1);
    const end = rest.search(/^#{2,3}\s/m);
    const section = end === -1 ? rest : rest.slice(0, end);
    for (const m of section.matchAll(/`([a-z][a-z0-9_]{2,})`/g)) known.add(m[1]);
  }

  if (known.size < 50) {
    throw new Error(
      `Harvested only ${known.size} keys from ${relative(docsRoot, referencePath)}: ` +
        "the reference format changed and this lint would produce nothing but false " +
        "positives. Fix the harvester before trusting a green run.",
    );
  }
  return known;
}

/** Fenced json5/json blocks in an MDX file, with their starting line. */
function extractSnippets(text) {
  const snippets = [];
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const fence = /^\s*```(json5|json)\b/.exec(lines[i]);
    if (!fence) continue;
    const prev = lines
      .slice(Math.max(0, i - 3), i)
      .join("\n");
    const ignored = prev.includes(IGNORE_MARKER);
    let j = i + 1;
    const body = [];
    while (j < lines.length && !/^\s*```\s*$/.test(lines[j])) body.push(lines[j++]);
    snippets.push({ startLine: i + 2, lang: fence[1], body: body.join("\n"), ignored });
    i = j;
  }
  return snippets;
}

/** Strip comments and string literals so brace/key scanning cannot be fooled. */
function stripNoise(src) {
  let out = "";
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === '"' || c === "'") {
      const quote = c;
      out += " ";
      i++;
      while (i < src.length && src[i] !== quote) {
        if (src[i] === "\\") i++;
        i++;
      }
      i++;
      continue;
    }
    if (c === "/" && src[i + 1] === "/") {
      while (i < src.length && src[i] !== "\n") i++;
      continue;
    }
    if (c === "/" && src[i + 1] === "*") {
      i += 2;
      while (i < src.length && !(src[i] === "*" && src[i + 1] === "/")) {
        if (src[i] === "\n") out += "\n";
        i++;
      }
      i += 2;
      continue;
    }
    out += c;
    i++;
  }
  return out;
}

/**
 * Bare object keys in a JSON5 fragment, with the stack of parent keys at the
 * point each appears. Works on fragments (no parse, no balanced-brace
 * requirement), so elided examples still get checked.
 */
function scanKeys(src) {
  const clean = stripNoise(src);
  const found = [];
  const stack = [];
  let pendingKey = null;
  let line = 1;

  const keyRe = /([A-Za-z_][A-Za-z0-9_]*)\s*:/y;
  for (let i = 0; i < clean.length; i++) {
    const c = clean[i];
    if (c === "\n") {
      line++;
      continue;
    }
    if (c === "{" || c === "[") {
      stack.push(pendingKey);
      pendingKey = null;
      continue;
    }
    if (c === "}" || c === "]") {
      stack.pop();
      pendingKey = null;
      continue;
    }
    if (/[A-Za-z_]/.test(c)) {
      keyRe.lastIndex = i;
      const m = keyRe.exec(clean);
      if (m) {
        const parents = stack.filter(Boolean);
        found.push({ key: m[1], line, parents });
        pendingKey = m[1];
        i = keyRe.lastIndex - 1;
        continue;
      }
      // Not a key; skip the whole identifier so `true`/`null` don't re-trigger.
      while (i < clean.length && /[A-Za-z0-9_]/.test(clean[i])) i++;
      i--;
    }
  }
  return found;
}

function braceBalance(src) {
  const clean = stripNoise(src);
  let curly = 0;
  let square = 0;
  for (const c of clean) {
    if (c === "{") curly++;
    else if (c === "}") curly--;
    else if (c === "[") square++;
    else if (c === "]") square--;
    if (curly < 0 || square < 0) return { ok: false, why: "closes a brace it never opened" };
  }
  if (curly !== 0) return { ok: false, why: `${curly > 0 ? curly : -curly} unmatched \`{\`/\`}\`` };
  if (square !== 0) return { ok: false, why: `${square > 0 ? square : -square} unmatched \`[\`/\`]\`` };
  return { ok: true };
}

// --------------------------------------------------------------------------

function main() {
  const known = harvestKnownKeys(REFERENCE);
  const files = walk(contentDir).filter((f) => !f.includes("reference/nativelink-config/"));

  const problems = [];
  let checked = 0;
  let skipped = 0;

  for (const abs of files) {
    const rel = relative(docsRoot, abs).split("\\").join("/");
    const text = readFileSync(abs, "utf8");
    for (const snip of extractSnippets(text)) {
      if (snip.ignored) {
        skipped++;
        continue;
      }
      checked++;

      const balance = braceBalance(snip.body);
      if (!balance.ok) {
        problems.push({
          file: rel,
          line: snip.startLine,
          message: `snippet is not brace-balanced: ${balance.why}`,
        });
        continue;
      }

      for (const { key, line, parents } of scanKeys(snip.body)) {
        if (parents.some((p) => FREEFORM_PARENTS.has(p))) continue;
        if (known.has(key)) continue;
        problems.push({
          file: rel,
          line: snip.startLine + line - 1,
          message: `\`${key}\` is not a field in the generated configuration reference${
            parents.length ? ` (under ${parents.map((p) => `\`${p}\``).join(" › ")})` : ""
          }`,
        });
      }
    }
  }

  console.log(
    `Checked ${checked} config snippet(s) across ${files.length} page(s) against ` +
      `${known.size} known keys${skipped ? `; skipped ${skipped} marked ignore` : ""}.`,
  );

  if (problems.length === 0) {
    console.log("No drift found.");
    return;
  }

  console.error(`\n${problems.length} problem(s):`);
  for (const p of problems) console.error(`  ${p.file}:${p.line}  ${p.message}`);
  console.error(
    "\nIf a snippet is deliberately not NativeLink config, put " +
      `\`{/* ${IGNORE_MARKER} */}\` on the line before its fence.`,
  );
  process.exitCode = 1;
}

main();
