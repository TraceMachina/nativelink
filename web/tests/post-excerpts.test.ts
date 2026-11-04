import { afterAll, describe, expect, test } from "@jest/globals";
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const root = mkdtempSync(path.join(tmpdir(), "post-excerpts-"));
const postsDir = path.join(root, "content/posts");
mkdirSync(postsDir, { recursive: true });
afterAll(() => rmSync(root, { recursive: true, force: true }));

const introduction =
  "An existing opening quotation remains the summary when an article has no explicit excerpt.";
for (const [slug, field] of [
  ["existing", ""],
  ["blank", 'excerpt: "   "'],
] as const) {
  writeFileSync(
    path.join(postsDir, `${slug}.md`),
    `---\ntitle: ${slug}\n${field}\n---\n\n> ${introduction}\n\nThe article body follows.`,
  );
}
writeFileSync(
  path.join(postsDir, "article.md"),
  readFileSync(
    new URL(
      "../apps/web/content/posts/The_Bomb_in_Your_Bill_of_Materials.md",
      import.meta.url,
    ),
  ),
);

// Run the public loader with an isolated working directory, as the website does.
const moduleUrl = new URL("../apps/web/lib/posts.ts", import.meta.url).href;
const result = spawnSync(
  process.execPath,
  [
    "-e",
    `const { getAllPosts } = await import(${JSON.stringify(moduleUrl)}); console.log(JSON.stringify(getAllPosts()));`,
  ],
  { cwd: root, encoding: "utf8" },
);
if (result.status !== 0) throw new Error(result.stderr);
const posts = JSON.parse(result.stdout) as {
  slug: string;
  excerpt: string;
  body: string;
}[];

describe("post excerpts", () => {
  test("uses the article's explicit summary instead of its lead image or quotation", () => {
    const post = posts.find(
      (item) => item.slug === "the-bomb-in-your-bill-of-materials",
    );
    expect(post?.excerpt).toBe(
      "Most software bombs don't go off the day they're dropped. A living SBOM and fast, reproducible verification help teams find vulnerabilities and ship fixes.",
    );
    expect(post?.body).toContain("<figure>");
    expect(post?.body).toContain("https://www.youtube.com/watch?v=17lkdqoLt44");
  });

  test("preserves automatic summaries for existing articles", () => {
    expect(posts.find((post) => post.slug === "existing")?.excerpt).toBe(
      introduction,
    );
  });

  test("treats a blank explicit excerpt as an automatic summary", () => {
    expect(posts.find((post) => post.slug === "blank")?.excerpt).toBe(
      introduction,
    );
  });
});
