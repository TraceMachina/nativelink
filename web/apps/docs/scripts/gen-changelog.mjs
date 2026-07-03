#!/usr/bin/env node
// Generate the changelog reference page from the repository's canonical
// CHANGELOG.md (maintained with git-cliff at release time; see /cliff.toml and
// the "Creating a Release" section of CONTRIBUTING.md).
//
// The output at content/docs/reference/changelog.md is gitignored and
// regenerated as part of the `dev` and `build` scripts, so the published page
// always mirrors the CHANGELOG.md of the commit being deployed: every merge to
// main produces a fresh page via the production build, and the web CI build on
// pull requests fails if a changelog edit stops compiling.
//
// The page is emitted as plain markdown (.md), which fumadocs compiles
// without JSX or ESM support: `{`, `}`, and statement-like lines such as
// `import x from "y"` in commit subjects are inert text there, while in .mdx
// each of them is syntax that can break the compile. The transform:
//   1. drop the static git-cliff header (everything before the first `## `),
//      replacing it with frontmatter and a short intro,
//   2. drop git-cliff's whole-line comment marker lines by exact string
//      comparison against the known markers (step 3 would otherwise escape
//      them into visible text),
//   3. backslash-escape `\` and `<` outside inline code spans and fenced
//      code blocks, so commit subjects can't smuggle raw HTML like
//      `<script>` into the page, and a literal backslash before a `<` can't
//      swallow the escape that neutralizes it.
//
// Step 3 is the security boundary: marker lines are removed by exact match —
// there is no comment-parsing pattern to circumvent — and every other `<`
// outside code reaches the page as escaped `\<` prose, so no HTML comment or
// tag can survive into the rendered page, however it is split across lines.
//
// Source resolution: CHANGELOG.md sits at the nativelink repository root,
// which is ABOVE the web/ workspace. Local checkouts and GitHub CI have it on
// disk, so we walk up from this script until we find it (next to cliff.toml,
// so an unrelated CHANGELOG.md can't be picked up). Vercel only mounts the
// workspace into the build container, so there we fetch the file from GitHub
// raw, pinned to the exact commit being deployed via Vercel's built-in
// VERCEL_GIT_* variables — same bytes, no drift.
//
// Usage from web/:
//   bun --filter @nativelink/docs gen:changelog

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const targetFile = join(here, "../content/docs/reference/changelog.md");

/**
 * Find CHANGELOG.md by walking up from this script towards the filesystem
 * root. Requiring cliff.toml as a sibling pins the match to the nativelink
 * repository root rather than any stray changelog on the way up.
 */
function findLocalChangelog() {
  for (let dir = here; ; ) {
    if (existsSync(join(dir, "cliff.toml")) && existsSync(join(dir, "CHANGELOG.md"))) {
      return join(dir, "CHANGELOG.md");
    }
    const parent = resolve(dir, "..");
    if (parent === dir) {
      return null;
    }
    dir = parent;
  }
}

/** Fetch CHANGELOG.md at the exact deployed commit from GitHub raw. */
async function fetchChangelogForDeployedCommit() {
  const owner = process.env.VERCEL_GIT_REPO_OWNER;
  const repo = process.env.VERCEL_GIT_REPO_SLUG;
  const sha = process.env.VERCEL_GIT_COMMIT_SHA;
  if (!owner || !repo || !sha) {
    return null;
  }
  const url = `https://raw.githubusercontent.com/${owner}/${repo}/${sha}/CHANGELOG.md`;
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`gen-changelog: fetching ${url} failed: HTTP ${response.status}`);
  }
  console.log(`gen-changelog: read CHANGELOG.md from ${url}`);
  return response.text();
}

async function readChangelog() {
  const localFile = findLocalChangelog();
  if (localFile) {
    return { text: readFileSync(localFile, "utf8"), origin: localFile };
  }
  const fetched = await fetchChangelogForDeployedCommit();
  if (fetched !== null) {
    return { text: fetched, origin: "GitHub raw (deployed commit)" };
  }
  throw new Error(
    [
      `gen-changelog: CHANGELOG.md not found next to a cliff.toml in any directory above ${here},`,
      "and the VERCEL_GIT_* variables needed to fetch it for the deployed commit are unset.",
      "Run from a nativelink checkout, or on Vercel enable 'Include source",
      "files outside of the Root Directory' so the repository root is available to the build.",
    ].join(" "),
  );
}

/**
 * Escape prose (text outside code) so it can't form raw HTML. Backslash is
 * in the set so a literal `\` in the source doubles instead of merging with
 * the `\` this inserts before a following `<`: input `\<` becomes `\\\<`
 * (escaped backslash, escaped `<`), never `\\<` (which would render a raw
 * `<`). Single pass, so the replacements can't interact.
 */
const escapeProse = (text) => text.replace(/[\\<]/g, (c) => `\\${c}`);

/**
 * Escape one markdown line, leaving single-backtick inline code spans
 * untouched. Odd split indices are the captured code spans. Double-backtick
 * spans would be over-escaped, but git-cliff commit subjects don't use them
 * and the failure mode is a stray backslash in rendered text, not a build
 * break.
 */
const escapeLine = (line) =>
  line
    .split(/(`[^`]*`)/)
    .map((segment, i) => (i % 2 === 1 ? segment : escapeProse(segment)))
    .join("");

/**
 * The exact whole-line comment markers git-cliff wraps the changelog in
 * (from the header/footer templates in /cliff.toml). Matching is by string
 * equality, not a pattern, so there is nothing to circumvent; any other
 * comment-like text is neutralized into escaped prose by escapeLine.
 */
const GIT_CLIFF_MARKER_LINES = new Set([
  "<!-- vale off -->",
  "<!-- generated by git-cliff -->",
  "<!-- vale on -->",
]);

const { text: raw, origin } = await readChangelog();

// Keep everything from the first release heading on; the git-cliff header
// (comment markers, title, "All notable changes..." line) is replaced by the
// intro below.
const firstHeading = raw.search(/^## /m);
if (firstHeading === -1) {
  throw new Error(`no "## " release heading found in ${origin}`);
}

let inFence = false;
const body = raw
  .slice(firstHeading)
  .split("\n")
  .map((line) => {
    if (/^\s{0,3}(```|~~~)/.test(line)) {
      inFence = !inFence;
      return line;
    }
    if (inFence) {
      return line;
    }
    return GIT_CLIFF_MARKER_LINES.has(line.trim()) ? null : escapeLine(line);
  })
  .filter((line) => line !== null)
  .join("\n")
  .trimEnd();

const page = `---
title: Changelog
description: Notable changes per release. Latest first.
---

<!-- Generated by scripts/gen-changelog.mjs — do not edit by hand. -->
<!-- The source of truth is CHANGELOG.md at the repository root. -->

This page mirrors
[\`CHANGELOG.md\`](https://github.com/TraceMachina/nativelink/blob/main/CHANGELOG.md),
the canonical changelog maintained with [git-cliff](https://git-cliff.org)
as part of each release.

${body}
`;

mkdirSync(dirname(targetFile), { recursive: true });
writeFileSync(targetFile, page);
console.log(`gen-changelog: wrote ${targetFile}`);
