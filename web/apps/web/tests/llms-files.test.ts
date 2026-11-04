import { expect, test } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  LLMS_FILES,
  approxTokens,
  formatBytes,
  formatTokens,
  llmsFileBytes,
  sizeLine,
} from "../lib/llms-files";

test("sizes print in decimal units, the way a browser reports a download", () => {
  expect(formatBytes(512)).toBe("512 B");
  expect(formatBytes(28_183)).toBe("28 KB");
  expect(formatBytes(221_861)).toBe("222 KB");
  expect(formatBytes(1_104_249)).toBe("1.1 MB");
});

test("token estimate is four bytes per token, rounded to thousands", () => {
  expect(approxTokens(28_183)).toBe(7046);
  expect(formatTokens(approxTokens(28_183))).toBe("7k tokens");
  expect(formatTokens(approxTokens(221_861))).toBe("55k tokens");
  expect(formatTokens(approxTokens(1_104_249))).toBe("276k tokens");
  expect(formatTokens(900)).toBe("900 tokens");
});

test("the size line is omitted, not invented, when the file is not built", () => {
  expect(sizeLine(null)).toBeNull();
  expect(sizeLine(221_861)).toBe("222 KB · about 55k tokens");
});

test("file sizes are read from disk, never typed by hand", () => {
  const dir = mkdtempSync(join(tmpdir(), "llms-"));
  writeFileSync(join(dir, "llms.txt"), "x".repeat(1234));
  expect(llmsFileBytes("llms.txt", dir)).toBe(1234);
  expect(llmsFileBytes("llms-full.txt", dir)).toBeNull();
});

test("the three files are listed in fetch order, index first", () => {
  expect(LLMS_FILES.map((f) => f.name)).toEqual(["llms.txt", "llms-small.txt", "llms-full.txt"]);
  expect(LLMS_FILES[0]?.when).toMatch(/first/);
  expect(LLMS_FILES[2]?.when).toMatch(/only when/);
});
