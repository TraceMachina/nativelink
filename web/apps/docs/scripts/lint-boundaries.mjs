#!/usr/bin/env node
// Boundary lint: the open-source docs stay self-contained.
//
// These docs make a promise: everything described here can be run by anyone
// from the source in this repository, with no account and nothing to buy. That
// promise is easy to make once and hard to keep, because the natural place to
// mention a hosted offering is exactly where a self-hosted path gets tedious,
// and each individual mention reads as helpful.
//
// The policy is not "never mention the hosted product". It is:
//
//   - Exactly one page, reference/oss-and-enterprise.mdx, compares the two and
//     is allowed to sell.
//   - Any other page may LINK to that page, and may state a factual boundary
//     ("this is not implemented in the open-source distribution").
//   - No other page may carry a call to action: no sign-up, no pricing, no
//     trial, no demo, no sales contact.
//
// So this lint looks for calls to action rather than for the words "cloud" or
// "enterprise", which have ordinary technical meanings this project uses
// constantly and which a naive grep would drown in.
//
// Escape hatch, on the line before the offending line:
//   {/* lint-boundaries: ignore (<why>) */}
// Use it for a genuine false positive and say why. It is not a way to add a
// call to action with extra steps.
//
// Usage, from web/:
//   bun --filter @nativelink/docs lint:boundaries
// or directly:
//   node scripts/lint-boundaries.mjs
//
// Exit code is 1 if any page outside the allowed one carries a call to action.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const docsRoot = join(here, "..");
const contentDir = join(docsRoot, "content/docs");

// The one page allowed to compare the open-source project with the hosted
// product, and to link outward for it.
const ALLOWED = new Set(["content/docs/reference/oss-and-enterprise.mdx"]);

const IGNORE = /\{\/\*\s*lint-boundaries:\s*ignore/;

const RULES = [
  {
    name: "sign-up call to action",
    re: /\b(sign\s?up|signup|create (?:a |an )?(?:free )?account|get started free|start (?:your )?free trial|try it free)\b/i,
  },
  {
    name: "pricing or plan language",
    re: /\b(free tier|paid tier|pricing page|per-seat|per seat|enterprise plan|pro plan|billing)\b/i,
  },
  {
    name: "sales or demo contact",
    re: /\b(contact (?:our )?sales|talk to (?:an? )?(?:expert|sales)|book a demo|schedule a demo|request a demo)\b/i,
  },
  {
    name: "hosted-product marketing link",
    re: /https?:\/\/(?:www\.)?nativelink\.com\/(?:pricing|signup|sign-up|contact|demo|dashboard|app)\b/i,
  },
  {
    name: "upsell framing",
    re: /\b(upgrade to (?:the )?(?:cloud|enterprise|pro)|available (?:only )?(?:on|in) (?:nativelink )?cloud\b)/i,
  },
];

// --------------------------------------------------------------------------

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (name.endsWith(".mdx")) out.push(full);
  }
  return out;
}

function main() {
  const files = walk(contentDir).sort();
  const problems = [];
  let checked = 0;
  let allowed = 0;

  for (const file of files) {
    const rel = relative(docsRoot, file).replace(/\\/g, "/");
    if (ALLOWED.has(rel)) {
      allowed += 1;
      continue;
    }
    checked += 1;

    const lines = readFileSync(file, "utf8").split("\n");
    for (let i = 0; i < lines.length; i += 1) {
      if (i > 0 && IGNORE.test(lines[i - 1])) continue;
      for (const rule of RULES) {
        const hit = lines[i].match(rule.re);
        if (hit) {
          problems.push({
            rel,
            line: i + 1,
            message: `${rule.name}: \`${hit[0]}\``,
          });
        }
      }
    }
  }

  if (problems.length === 0) {
    console.log(
      `lint:boundaries: ${checked} pages carry no call to action (${allowed} page exempt).`,
    );
    return;
  }

  console.error(
    `lint:boundaries: ${problems.length} call(s) to action outside the one allowed page:\n`,
  );
  for (const { rel, line, message } of problems) {
    console.error(`  ${rel}:${line}  ${message}`);
  }
  console.error(
    "\nThese docs promise that everything in them can be run from this repository\n" +
      "with no account. Comparisons with the hosted product belong on\n" +
      "reference/oss-and-enterprise.mdx; every other page may link to it and state a\n" +
      "factual boundary, but may not sell.\n",
  );
  process.exit(1);
}

main();
