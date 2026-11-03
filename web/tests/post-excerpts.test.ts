import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { deriveExcerpt } from "../apps/web/lib/posts";

describe("deriveExcerpt", () => {
  test("skips lead figures and quotations to find the opening paragraph", () => {
    const opening =
      "A dependency inventory helps maintainers identify vulnerable components and verify their fixes.";
    const body = `**Author**

<figure>
  <img src="/assets/dependencies.svg" alt="A highlighted dependency in a software inventory." />
  <figcaption>This illustration has a caption long enough to be mistaken for the article summary.</figcaption>
</figure>

> This opening quotation is also long enough to be mistaken for the article summary.
>
> — Attribution

${opening}`;

    expect(deriveExcerpt(body)).toBe(opening);
  });

  test("strips HTML without leaving tag fragments in the summary", () => {
    const prose =
      "A declared dependency tree makes it easier to trace the components included in a release.";
    expect(deriveExcerpt(`<p><strong>${prose}</strong><br /></p>`)).toBe(prose);
  });

  test("preserves link text and the excerpt length limit", () => {
    const prose =
      "Review the dependency inventory, patch affected components, and verify the result. ".repeat(
        4,
      );
    const expected = `Read the inventory. ${prose}`;
    expect(deriveExcerpt(`[Read the inventory](https://example.com). ${prose}`)).toBe(
      `${expected.slice(0, 197).trimEnd()}…`,
    );
  });

  test("summarizes the bill of materials article from its introduction", () => {
    const article = readFileSync(
      new URL("../apps/web/content/posts/The_Bomb_in_Your_Bill_of_Materials.md", import.meta.url),
      "utf8",
    );
    const body = article.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n?/, "");
    const opening = body
      .split(/\r?\n\r?\n/)
      .find((block) => block.startsWith("Most software bombs"));
    expect(opening).toBeDefined();
    expect(deriveExcerpt(body)).toBe(`${opening?.slice(0, 197).trimEnd()}…`);
  });
});
