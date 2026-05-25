import type { ViteUserConfig } from "astro";
import { defineConfig, passthroughImageService } from "astro/config";

import { rehypeHeadingIds } from "@astrojs/markdown-remark";
import react from "@astrojs/react";
import sitemap from "@astrojs/sitemap";
import starlight from "@astrojs/starlight";

import cloudflare from "@astrojs/cloudflare";
import partytown from "@astrojs/partytown";
import qwik from "@qwikdev/astro";
import tailwindcss from "@tailwindcss/vite";

import { rehypeMermaid } from "@beoe/rehype-mermaid"; // "rehype-mermaid";
import rehypeAutolinkHeadings from "rehype-autolink-headings";

import { starlightConfig } from "./starlight.conf";

const tailwindPlugins = tailwindcss() as unknown as NonNullable<
  ViteUserConfig["plugins"]
>;

// https://astro.build/config
export default defineConfig({
  site: "https://nativelink.com",
  output: "server",
  image: {
    service: passthroughImageService(),
  },
  adapter: cloudflare(),
  redirects: {
    "/blog/case-study%3A-samsung-internet's-integration-with-nativelink": {
      status: 301,
      destination: "/resources/blog",
    },
    "/resources/blog/case-study-samsung": {
      status: 301,
      destination: "/resources/blog",
    },
  },
  integrations: [
    qwik({
      include: ["**/components/qwik/**/*"],
    }),
    react({
      include: ["**/components/react/*"],
    }),
    starlight(starlightConfig),
    sitemap(),
    partytown({
      config: {
        loadScriptsOnMainThread: [
          "www.youtube-nocookie.com",
          "www.youtube.com",
        ],
        forward: ["dataLayer.push"],
      },
    }),
  ],
  markdown: {
    rehypePlugins: [
      rehypeHeadingIds,
      [
        rehypeAutolinkHeadings,
        {
          behavior: "wrap",
        },
      ],
      [
        rehypeMermaid,
        {
          darkScheme: "class",
          strategy: "data-url",
        },
      ],
    ],
  },
  vite: {
    plugins: tailwindPlugins,
  },
});
