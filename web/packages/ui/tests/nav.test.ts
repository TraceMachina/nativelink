import { expect, test } from "bun:test";
import { isCurrentNavHref, normalizeNavPath } from "../src/lib/nav";

test("normalizeNavPath strips query, hash, and trailing slashes", () => {
  expect(normalizeNavPath("/product/")).toBe("/product");
  expect(normalizeNavPath("/product?ref=nav")).toBe("/product");
  expect(normalizeNavPath("/product#hero")).toBe("/product");
  expect(normalizeNavPath("/")).toBe("/");
  expect(normalizeNavPath("/?utm=1")).toBe("/");
});

test("home is exact-only", () => {
  expect(isCurrentNavHref("/", "/")).toBe(true);
  expect(isCurrentNavHref("/", "/product")).toBe(false);
  expect(isCurrentNavHref("/", "/resources/blog/slug")).toBe(false);
});

test("section links match themselves and nested routes", () => {
  expect(isCurrentNavHref("/product", "/product")).toBe(true);
  expect(isCurrentNavHref("/resources", "/resources/blog/hello")).toBe(true);
  expect(isCurrentNavHref("/product", "/pricing")).toBe(false);
  expect(isCurrentNavHref("/resources", "/resource")).toBe(false);
});

test("external and missing paths never match", () => {
  expect(isCurrentNavHref("https://enterprise.nativelink.com", "/")).toBe(false);
  expect(isCurrentNavHref("/docs", undefined)).toBe(false);
  expect(isCurrentNavHref("mailto:hello@nativelink.com", "/")).toBe(false);
});
