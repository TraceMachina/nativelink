import { expect, test } from "@jest/globals";
import {
  TIER_KEYS,
  collapseUniversalRows,
  comparison,
  isIdenticalAcrossTiers,
  isUniversal,
} from "../lib/pricing";

test("features every tier includes leave the comparison rows", () => {
  const { universal, differentiators } = collapseUniversalRows(comparison);
  expect(universal).toEqual([
    "Unlimited cores, machines and users",
    "Distributed scheduler",
    "Remote caching",
    "Remote execution",
    "Cross-compilation",
    "Deterministic, verifiable builds",
    "External storage (S3, GCS, Redis)",
    "All supported build systems",
  ]);
  for (const group of differentiators) {
    for (const row of group.rows) {
      expect(isUniversal(row)).toBe(false);
    }
  }
});

test("every remaining row really differs between the tiers", () => {
  // The table intro says every row below the summary marks a difference.
  const { differentiators } = collapseUniversalRows(comparison);
  for (const group of differentiators) {
    for (const row of group.rows) {
      expect(isIdenticalAcrossTiers(row)).toBe(false);
    }
  }
});

test("the live table keeps every section, in order", () => {
  const { differentiators } = collapseUniversalRows(comparison);
  expect(differentiators.map((g) => g.section)).toEqual([
    "Platform",
    "Compatibility",
    "Operations",
    "Support",
    "Licence",
  ]);
});

test("Tier 2 includes everything Tier 1 includes", () => {
  for (const group of comparison) {
    for (const row of group.rows) {
      if (row.t1 === true) {
        expect(row.t2).toBe(true);
      }
    }
  }
});

test("a section left with no differentiators is dropped", () => {
  const { universal, differentiators } = collapseUniversalRows([
    { section: "A", rows: [{ label: "x", oss: true, t1: true, t2: true }] },
    { section: "B", rows: [{ label: "y", oss: false, t1: true, t2: true }] },
  ]);
  expect(universal).toEqual(["x"]);
  expect(differentiators).toEqual([
    { section: "B", rows: [{ label: "y", oss: false, t1: true, t2: true }] },
  ]);
});

test("text and link values are never collapsed, even when they match", () => {
  expect(TIER_KEYS).toEqual(["oss", "t1", "t2"]);
  expect(
    isUniversal({ label: "Build systems", oss: "All", t1: "All", t2: "All" }),
  ).toBe(false);
  const link = { text: "Portal", href: "https://example.com" };
  expect(isUniversal({ label: "Hosting", oss: link, t1: link, t2: link })).toBe(
    false,
  );
  expect(
    isIdenticalAcrossTiers({ label: "Hosting", oss: link, t1: link, t2: link }),
  ).toBe(true);
});
