import { expect, test } from "bun:test";
import { SEE_PRICING, TALK_TO_US, closingSecondaryCta } from "../lib/cta";

test("closing band points at pricing from everywhere else", () => {
  expect(closingSecondaryCta("/")).toBe(SEE_PRICING);
  expect(closingSecondaryCta("/product")).toBe(SEE_PRICING);
  expect(closingSecondaryCta("/contact")).toBe(SEE_PRICING);
  expect(closingSecondaryCta(null)).toBe(SEE_PRICING);
  expect(closingSecondaryCta(undefined)).toBe(SEE_PRICING);
});

test("closing band never links Pricing to itself", () => {
  expect(closingSecondaryCta("/pricing")).toBe(TALK_TO_US);
  expect(closingSecondaryCta("/pricing/")).toBe(TALK_TO_US);
  expect(closingSecondaryCta("/pricing?plan=enterprise")).toBe(TALK_TO_US);
  expect(closingSecondaryCta("/pricing#compare")).toBe(TALK_TO_US);
});
