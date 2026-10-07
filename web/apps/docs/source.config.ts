import { defineDocs, defineConfig } from "fumadocs-mdx/config";
import { rehypeCodeDefaultOptions } from "fumadocs-core/mdx-plugins";

export const docs = defineDocs({
  dir: "content/docs",
});

export default defineConfig({
  mdxOptions: {
    rehypeCodeOptions: {
      ...rehypeCodeDefaultOptions,
      // Shiki has no `starlark` grammar. Starlark is a Python dialect, so load
      // Python and highlight `starlark` blocks (Bazel BUILD/.bzl) with it.
      langs: ["python"],
      langAlias: {
        starlark: "python",
      },
    },
  },
});
