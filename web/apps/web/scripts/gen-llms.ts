#!/usr/bin/env bun
// Generate the marketing site's llms files, and the repository's.
//
//   public/llms.txt, public/llms-small.txt, public/llms-full.txt
//     what nativelink.com serves; gitignored and rebuilt by `dev` and `build`.
//   <repo root>/llms.txt, llms-small.txt, llms-full.txt
//     the same bytes, committed, so a clone of the repository carries its own
//     corpus. Written only when the repository root is on disk (it is not in a
//     Vercel build, which mounts the web/ workspace alone), and kept current by
//     the "Regenerate llms files" workflow.
//
// Sources, in order of appearance in the output:
//   lib/llms.ts                      the site's pages in prose and the standing sections
//   content/posts/*.mdx              the blog, via lib/posts.ts
//   ../docs/scripts/gen-llms.mjs     the documentation corpus, built from the docs nav
//
// Usage, from web/:
//   bun --filter @nativelink/web gen:llms

import { spawnSync } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildSections, renderFull, renderSmall } from "../../docs/scripts/gen-llms.mjs";
import type { DocsCorpus } from "../lib/llms";

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(here, "..");
const docsRoot = resolve(appRoot, "../docs");
const publicDir = join(appRoot, "public");

// lib/posts.ts resolves content/posts against the working directory when it
// is first evaluated, so change directory before importing the site module.
process.chdir(appRoot);
const site = await import("../lib/llms");

// The docs nav lists the changelog page, which is itself generated; the docs
// generator fails loudly on a nav entry with no page behind it.
const changelog = join(docsRoot, "content/docs/reference/changelog.md");
if (!existsSync(changelog)) {
  const result = spawnSync(process.execPath, [join(docsRoot, "scripts/gen-changelog.mjs")], {
    stdio: "inherit",
  });
  if (result.status !== 0) {
    throw new Error("gen-llms: could not generate the docs changelog page first");
  }
}

/** The repository root, found by the file only it has; null in a workspace-only checkout. */
function findRepoRoot(from: string): string | null {
  let dir = from;
  for (let i = 0; i < 8; i += 1) {
    if (existsSync(join(dir, "cliff.toml")) && existsSync(join(dir, "AGENTS.md"))) return dir;
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

const sections = buildSections();
const docs: DocsCorpus = {
  sections,
  small: renderSmall(sections),
  full: renderFull(sections),
};
const outputs: [string, string][] = [
  ["llms.txt", site.renderIndex(docs)],
  ["llms-small.txt", site.renderSmall(docs)],
  ["llms-full.txt", site.renderFull(docs)],
];

const targets = [publicDir];
const repoRoot = findRepoRoot(appRoot);
if (repoRoot) targets.push(repoRoot);

for (const dir of targets) {
  for (const [name, text] of outputs) {
    writeFileSync(join(dir, name), text);
  }
}

const sizes = outputs
  .map(([name, text]) => `${name} ${Math.round(text.length / 1024)} KB`)
  .join(", ");
console.log(
  `gen-llms: wrote ${sizes} to public/${repoRoot ? " and the repository root" : " (repository root not on disk; skipped)"}.`,
);
