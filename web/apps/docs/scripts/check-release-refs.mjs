#!/usr/bin/env node
/**
 * Checks that the release references in the README and the docs work.
 *
 * Quickstart snippets look up the newest release when they run and put it
 * in NATIVELINK_VERSION, then build URLs from it:
 *
 *   raw.githubusercontent.com/TraceMachina/nativelink/[refs/tags/]${NATIVELINK_VERSION}/...
 *   ghcr.io/tracemachina/nativelink:${NATIVELINK_VERSION}
 *   github:TraceMachina/nativelink/${NATIVELINK_VERSION}      (nix run)
 *
 * The same shapes with a literal version (v1.2.3) are hard-coded pins, and
 * they are not allowed: a pin goes stale with the next release, and a pin
 * without the `v` that tags carry is a 404 on day one, which is how the
 * README quickstart once broke for every new user.
 *
 * Usage, from anywhere in the repository:
 *
 *   node web/apps/docs/scripts/check-release-refs.mjs
 *       Static check, no network: no hard-coded pins, and every file that
 *       uses NATIVELINK_VERSION also sets it (the lookup, or an explicit
 *       vX.Y.Z placeholder for a version the reader chooses).
 *
 *   node web/apps/docs/scripts/check-release-refs.mjs --resolve [--wait <seconds>]
 *       Also fetch every URL and image manifest, with NATIVELINK_VERSION
 *       set to the latest published release, and fail on anything that is
 *       not a 200. This is what catches a release that renamed a config the
 *       quickstarts download. `--wait` retries failures for up to that long,
 *       for use right after a release while the image is still publishing.
 *
 * No dependencies beyond Node 20, so CI can run it without installing the
 * web workspace.
 */

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const REPO = "TraceMachina/nativelink";
const IMAGE = "tracemachina/nativelink";
// A literal version, or a reference to the variable the snippets set.
const VERSION = String.raw`v?\d+\.\d+\.\d+|\$\{?NATIVELINK_VERSION\}?`;
const RAW_URL = String.raw`https://raw\.githubusercontent\.com/${REPO}/(?:refs/tags/)?(${VERSION})/[^\s"'\`)]+`;

// Each pattern captures the version in group 1. The surrounding text is what
// makes a match a pin rather than, say, a changelog heading.
const PATTERNS = [
  { kind: "url", re: new RegExp(RAW_URL, "g") },
  {
    kind: "image",
    re: new RegExp(String.raw`ghcr\.io/${IMAGE}:(${VERSION})`, "g"),
  },
  {
    kind: "flake",
    re: new RegExp(String.raw`github:${REPO}/(${VERSION})`, "g"),
  },
];

// Every request gets a deadline, so one stalled connection fails (and is
// retried) instead of hanging the job until its own timeout.
const timeout = () => AbortSignal.timeout(30_000);
const HEAD = { method: "HEAD", redirect: "follow" };

// Generated pages pin their own ref (the release they were generated from)
// and are rewritten by their generator.
const EXCLUDE = [
  "web/apps/docs/content/docs/reference/nativelink-config/",
  "web/apps/docs/content/docs/reference/changelog.md",
  "web/apps/docs/content/docs/reference/metrics.mdx",
];

const root = execFileSync("git", ["rev-parse", "--show-toplevel"], {
  encoding: "utf8",
}).trim();

function trackedFiles() {
  const args = [
    "ls-files",
    "README.md",
    "web/apps/docs/content/docs/*.md",
    "web/apps/docs/content/docs/*.mdx",
    "web/apps/web/content/*.md",
    "web/apps/web/content/*.mdx",
  ];
  const out = execFileSync("git", args, { cwd: root, encoding: "utf8" });
  return out
    .split("\n")
    .filter(Boolean)
    .filter((f) => !EXCLUDE.some((x) => f.startsWith(x)));
}

// `NATIVELINK_VERSION=` in a shell, `$NATIVELINK_VERSION =` in PowerShell.
const ASSIGNS = /(^|\s)\$?NATIVELINK_VERSION\s?=/m;

function findPins(files) {
  const pins = [];
  for (const file of files) {
    const text = readFileSync(join(root, file), "utf8");
    const assigns = ASSIGNS.test(text);
    const lines = text.split("\n");
    lines.forEach((line, i) => {
      for (const { kind, re } of PATTERNS) {
        for (const m of line.matchAll(re)) {
          const dynamic = m[1].includes("NATIVELINK_VERSION");
          pins.push({
            file,
            line: i + 1,
            kind,
            version: m[1],
            text: m[0],
            dynamic,
            assigns,
          });
        }
      }
    });
  }
  return pins;
}

function staticCheck(pins) {
  const errors = [];
  for (const p of pins.filter((p) => !p.dynamic)) {
    const fix = "look it up into NATIVELINK_VERSION, as the quickstarts do";
    errors.push(`${p.file}:${p.line}: hard-coded ${p.text}\n  ${fix}`);
  }
  const unset = new Set();
  for (const p of pins.filter((p) => p.dynamic && !p.assigns)) {
    unset.add(p.file);
  }
  for (const file of unset) {
    const fix = "add the lookup the quickstarts use, before the first use";
    errors.push(`${file}: uses NATIVELINK_VERSION but never sets it\n  ${fix}`);
  }
  return errors;
}

// The same lookup the snippets do: /releases/latest redirects to the tag.
async function latestRelease() {
  const url = `https://github.com/${REPO}/releases/latest`;
  const res = await fetch(url, { ...HEAD, signal: timeout() });
  const tag = res.url.split("/").pop();
  if (!/^v\d+\.\d+\.\d+$/.test(tag)) {
    throw new Error(`could not resolve the latest release from ${res.url}`);
  }
  return tag;
}

async function ghcrManifestStatus(tag) {
  const tokenUrl = `https://ghcr.io/token?scope=repository:${IMAGE}:pull`;
  const tokenRes = await fetch(tokenUrl, { signal: timeout() });
  const { token } = await tokenRes.json();
  const res = await fetch(`https://ghcr.io/v2/${IMAGE}/manifests/${tag}`, {
    method: "HEAD",
    signal: timeout(),
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: [
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.docker.distribution.manifest.v2+json",
      ].join(","),
    },
  });
  return res.status;
}

// Every URL comes from a committed doc, but check the host anyway, so a
// typo or a crafted path can never point this at anything else.
const HOSTS = new Set(["raw.githubusercontent.com", "github.com"]);

async function urlStatus(url) {
  const { host, pathname } = new URL(url);
  if (!HOSTS.has(host) || !pathname.startsWith(`/${REPO}/`)) {
    return `refusing to fetch ${url}`;
  }
  const res = await fetch(url, { ...HEAD, signal: timeout() });
  return res.status;
}

async function resolveOnce(pins, latest) {
  const targets = new Map();
  for (const p of pins) {
    const version = p.dynamic ? latest : p.version;
    const text = p.text.replace(p.version, version);
    if (p.kind === "url") {
      targets.set(text, { label: text, check: () => urlStatus(text) });
    } else if (p.kind === "image") {
      const check = () => ghcrManifestStatus(version);
      targets.set(text, { label: text, check });
    } else {
      // A flake ref names a git tag.
      const url = `https://github.com/${REPO}/releases/tag/${version}`;
      const check = () => urlStatus(url);
      targets.set(url, { label: `tag ${version}`, check });
    }
  }
  const failures = [];
  for (const { label, check } of targets.values()) {
    let status;
    try {
      status = await check();
    } catch (e) {
      status = String(e);
    }
    console.log(`${status} ${label}`);
    if (status !== 200) failures.push(`${status} ${label}`);
  }
  return failures;
}

function arg(name) {
  const i = process.argv.indexOf(name);
  return i === -1 ? undefined : (process.argv[i + 1] ?? "");
}

async function main() {
  const pins = findPins(trackedFiles());
  if (pins.length === 0) {
    console.error("no release references found; the patterns are wrong");
    process.exit(1);
  }
  const dynamicCount = pins.filter((p) => p.dynamic).length;
  const fileCount = new Set(pins.map((p) => p.file)).size;
  console.log(`${pins.length} references in ${fileCount} files`);
  console.log(`${dynamicCount} of them use NATIVELINK_VERSION`);
  const errors = staticCheck(pins);

  if (errors.length === 0 && process.argv.includes("--resolve")) {
    const latest = await latestRelease();
    console.log(`latest release: ${latest}`);
    const deadline = Date.now() + Number(arg("--wait") ?? 0) * 1000;
    let failures = await resolveOnce(pins, latest);
    while (failures.length > 0 && Date.now() < deadline) {
      console.log(`${failures.length} not yet resolvable; retrying in 60s`);
      await new Promise((r) => setTimeout(r, 60_000));
      failures = await resolveOnce(pins, latest);
    }
    for (const f of failures) errors.push(`does not resolve: ${f}`);
  }

  if (errors.length > 0) {
    for (const e of errors) console.error(`error: ${e}`);
    process.exit(1);
  }
  console.log("ok");
}

await main();
