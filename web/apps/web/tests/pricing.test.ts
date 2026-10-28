import { expect, test } from "bun:test";
import { collapseUniversalRows, comparison, isUniversal } from "../lib/pricing";

test("features every tier includes leave the comparison rows", () => {
  const { universal, differentiators } = collapseUniversalRows(comparison);
  expect(universal).toEqual([
    "Distributed scheduler",
    "Remote caching",
    "Remote execution",
    "Cross-compilation",
    "External storage (S3, Redis)",
    "Org-wide sharing",
  ]);
  for (const group of differentiators) {
    for (const row of group.rows) {
      expect(isUniversal(row)).toBe(false);
    }
  }
});

test("every remaining row really differs between the tiers", () => {
  // The table intro says every row below the summary marks a difference.
  // isUniversal only collapses true/true, so keep the data honest here: no row
  // may show the same text or the same "not included" mark in both columns.
  const { differentiators } = collapseUniversalRows(comparison);
  for (const group of differentiators) {
    for (const row of group.rows) {
      expect(JSON.stringify(row.oss)).not.toBe(JSON.stringify(row.ent));
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
  ]);
});

test("a section left with no differentiators is dropped", () => {
  const { universal, differentiators } = collapseUniversalRows([
    { section: "A", rows: [{ label: "x", oss: true, ent: true }] },
    { section: "B", rows: [{ label: "y", oss: false, ent: true }] },
  ]);
  expect(universal).toEqual(["x"]);
  expect(differentiators).toEqual([
    { section: "B", rows: [{ label: "y", oss: false, ent: true }] },
  ]);
});

test("text and link values are never collapsed, even when they match", () => {
  expect(isUniversal({ label: "Build systems", oss: "All", ent: "All" })).toBe(false);
  expect(isUniversal({ label: "Autoscaling", oss: false, ent: true })).toBe(false);
  const link = { text: "Portal", href: "https://example.com" };
  expect(isUniversal({ label: "Hosting", oss: link, ent: link })).toBe(false);
});
