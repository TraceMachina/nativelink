import { expect, test } from "bun:test";
import { defaultFooterColumns } from "../src/components/site-footer";
import { defaultNavLinks } from "../src/components/site-header";
import { leavesSite } from "../src/lib/nav";

const footerLinks = defaultFooterColumns.flatMap((column) => column.links);

test("every header destination appears in the footer under the same label", () => {
  const footerLabelByHref = new Map(footerLinks.map((link) => [link.href, link.label]));
  for (const link of defaultNavLinks) {
    expect(footerLabelByHref.get(link.href)).toBe(link.label);
  }
});

test("no destination appears twice in the footer, and no label points two ways", () => {
  const hrefs = footerLinks.map((link) => link.href);
  expect(new Set(hrefs).size).toBe(hrefs.length);
  const labels = footerLinks.map((link) => link.label);
  expect(new Set(labels).size).toBe(labels.length);
});

test("links that leave the site sit at the end of the header", () => {
  const firstExternal = defaultNavLinks.findIndex((link) => leavesSite(link.href));
  expect(firstExternal).toBeGreaterThan(0);
  for (const link of defaultNavLinks.slice(firstExternal)) {
    expect(leavesSite(link.href)).toBe(true);
  }
});
