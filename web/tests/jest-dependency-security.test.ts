import * as fs from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout } from "node:timers/promises";
import { expect, test } from "@jest/globals";

// Exercise the dependencies Jest actually loads, rather than separate copies
// added solely for these tests.
const require = createRequire(import.meta.url);
const fromJest = createRequire(require.resolve("jest"));
const fromCore = createRequire(fromJest.resolve("@jest/core"));
const fromConfig = createRequire(fromCore.resolve("jest-config"));
const fromGlob = createRequire(fromConfig.resolve("glob"));
const fromTransform = createRequire(fromCore.resolve("@jest/transform"));
const fromIstanbul = createRequire(fromTransform.resolve("babel-plugin-istanbul"));
const fromExclude = createRequire(fromIstanbul.resolve("test-exclude"));

test.each([
  ["jest-config", fromConfig("glob").glob],
  ["test-exclude", fromExclude("glob").glob],
])("%s stops reading directories after cancellation", async (_name, glob) => {
  // GHSA-3g3m-2fjr-6pw4: rejection alone must also stop the filesystem walk.
  const cwd = fs.mkdtempSync(join(tmpdir(), "nativelink-glob-abort-"));
  try {
    fs.mkdirSync(join(cwd, "a", "b", "c"), { recursive: true });
    fs.writeFileSync(join(cwd, "a", "b", "c", "file.txt"), "fixture");
    const controller = new AbortController();
    const reason = new Error("cancel traversal");
    let reads = 0;
    const countedFs = {
      ...fs,
      readdir: (...args: Parameters<typeof fs.readdir>) => {
        if (++reads === 1) controller.abort(reason);
        return fs.readdir(...args);
      },
    };

    const walk = glob("**", { cwd, signal: controller.signal, fs: countedFs });
    await expect(walk).rejects.toBe(reason);
    // Allow the in-flight read to finish and expose any continued recursion.
    await setTimeout(100);
    expect(reads).toBe(1);

    expect(await glob("**/*.txt", { cwd })).toEqual(["a/b/c/file.txt"]);
  } finally {
    fs.rmSync(cwd, { recursive: true, force: true });
  }
});

test.each([
  ["glob", fromGlob("minimatch").Minimatch],
  ["test-exclude", fromExclude("minimatch").Minimatch],
])("%s rejects excessive globstar-parent pattern expansion", (_name, Minimatch) => {
  // GHSA-g3hv-75wg-m249: a small input must honor the expansion budget.
  const pattern = Array(8).fill("**/../a/b").join("/");
  const options = { optimizationLevel: 2, braceExpandMax: 64 };
  expect(() => new Minimatch(pattern, options)).toThrow("too many patterns");

  const ordinary = new Minimatch("**/../a/b/**/../c/d", {
    optimizationLevel: 2,
  });
  expect(ordinary.match("../a/b/../c/d")).toBe(true);
  expect(ordinary.match("x/a/b/y/c/d")).toBe(true);
  expect(ordinary.match("x/a/b/y/c/other")).toBe(false);
});
