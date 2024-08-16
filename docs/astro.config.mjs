import { rehypeHeadingIds } from "@astrojs/markdown-remark";
// import cloudflare from "@astrojs/cloudflare";
import partytown from "@astrojs/partytown";
import sitemap from "@astrojs/sitemap";
import starlight from "@astrojs/starlight";
import { rehypeMermaid } from "@beoe/rehype-mermaid"; // "rehype-mermaid";
import starlightUtils from "@lorenzo_lewis/starlight-utils";
import { default as playformCompress } from "@playform/compress";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";
import rehypeAutolinkHeadings from "rehype-autolink-headings";
import { visualizer } from "rollup-plugin-visualizer";
import { visit } from "unist-util-visit";

function rehypeLazyLoadMermaid() {
  return (tree) => {
    visit(tree, "element", (node) => {
      if (node.tagName === "img") {
        node.properties.loading = "lazy";
      }
    });
  };
}

// https://astro.build/config
// biome-ignore lint/style/noDefaultExport: Astro expects a default export.
export default defineConfig({
  // TODO(aaronmondal): Regularly test whether this still works. We currently
  //                    use a static build due to excessive SSR bundle size
  //                    caused by shiki. Migrate to full SSR once that's fixed.
  // output: "server",
  // adapter: cloudflare({
  //   imageService: "passthrough",
  //   routes: {
  //     extend: {
  //       exclude: [{ pattern: "/build/*" }, { pattern: "/pagefind/*" }],
  //     },
  //   },
  // }),
  markdown: {
    rehypePlugins: [
      rehypeHeadingIds,
      [rehypeAutolinkHeadings, { behavior: "wrap" }],
      [
        rehypeMermaid,
        // TODO(aaronmondal): The "@beoe/cache" package doesn't build on
        // Cloudflare. Reimplement our own.
        { class: "not-content", strategy: "img-class-dark-mode" },
      ],
      rehypeLazyLoadMermaid,
    ],
  },
  vite: {
    plugins: [visualizer()],
    css: {
      transformer: "lightningcss",
      plugins: [tailwindcss()],
    },
  },
  site: "https://nativelink.pages.dev",
  integrations: [
    partytown(),
    sitemap(),
    starlight({
      components: {
        PageFrame: "./src/components/PageFrame.astro",
      },
      logo: {
        light: "/src/assets/logo-light.svg",
        dark: "/src/assets/logo-dark.svg",
        replacesTitle: true,
      },
      title: "NativeLink Docs",
      social: {
        github: "https://github.com/TraceMachina/nativelink",
        slack:
          "https://nativelink.slack.com/join/shared_invite/zt-281qk1ho0-krT7HfTUIYfQMdwflRuq7A",
      },
      customCss: ["./src/assets/landing.css", "./src/assets/custom.css"],
      plugins: [
        starlightUtils({
          navLinks: {
            leading: { useSidebarLabelled: "leadingNavLinks" },
          },
        }),
      ],
      sidebar: [
        // The documentation structure follows the Diátaxis framework.
        // See https://diataxis.fr/ for details.
        {
          label: "Getting Started",
          items: [
            {
              label: "Introduction",
              link: "/introduction/setup",
            },
            {
              label: "NativeLink On-Prem",
              link: "/introduction/on-prem",
            },
            {
              label: "Other Build Systems",
              link: "/introduction/non-bre",
            },
          ],
        },
        {
          // Corresponds to https://diataxis.fr/tutorials/. Learning-oriented
          // content without elaborate explanations. Tutorials should have a
          // clear goal and a straightforward "follow-these-commands" structure.
          label: "NativeLink Cloud",
          items: [
            {
              label: "Bazel",
              link: "/nativelink-cloud/bazel/",
            },
            {
              label: "Reclient",
              link: "/nativelink-cloud/reclient/",
            },
            {
              label: "Pants",
              link: "/nativelink-cloud/pants/",
            },
            {
              label: "API Keys in CI",
              link: "/nativelink-cloud/api-key/",
            },
          ],
        },
        {
          // Corresponds to https://diataxis.fr/how-to-guides/. Guides don't
          // need to be "complete". They should provide practical guidance for
          // real-world use-cases.
          label: "Configuring NativeLink",
          items: [
            {
              label: "Configuration Introduction",
              link: "/config/configuration-intro",
            },
            {
              label: "Basic Configurations",
              link: "/config/basic-configs",
            },
            {
              label: "Production Configurations",
              link: "/config/production-config",
            },
          ],
        },
        {
          // Corresponds to https://diataxis.fr/how-to-guides/. Guides don't
          // need to be "complete". They should provide practical guidance for
          // real-world use-cases.
          label: "On-Prem Examples",
          items: [
            {
              label: "On-Prem Overview",
              link: "/deployment-examples/on-prem-overview/",
            },
            {
              label: "Kubernetes",
              link: "/deployment-examples/kubernetes/",
            },
            {
              label: "Chromium",
              link: "/deployment-examples/chromium/",
            },
          ],
        },
        {
          // Corresponds to https://diataxis.fr/explanation/. Information on
          // internal functionality and design concepts. Explanations should
          // explain design decisions, constraints, etc.
          label: "Understanding NativeLink",
          items: [
            {
              label: "Architecture",
              link: "/explanations/architecture/",
            },
            {
              label: "History",
              link: "/explanations/history/",
            },
            {
              label: "Local Remote Execution",
              link: "/explanations/lre/",
            },
          ],
        },
        {
          // Corresponds to https://diataxis.fr/explanation/. Addresses
          // common questions and confusions about esoteric tooling and
          // concepts. It aims to help new users feel more at ease and
          label: "FAQ",
          items: [
            {
              label: "Is NativeLink Free?",
              link: "/faq/cost",
            },
            {
              label: "What is Remote Caching?",
              link: "/faq/caching",
            },
            {
              label: "What is Remote Execution?",
              link: "/faq/remote-execution",
            },
            {
              label: "What is LRE?",
              link: "/faq/lre",
            },
            {
              label: "What are Toolchains?",
              link: "/faq/toolchains",
            },
            {
              label: "How do I make my Bazel setup hermetic?",
              link: "/faq/hermeticity",
            },
            {
              label: "What is Nix?",
              link: "/faq/nix",
            },
            {
              label: "Why Rust?",
              link: "/faq/rust",
            },
          ],
        },
        {
          // Corresponds to https://diataxis.fr/how-to-guides/. Guides for
          // contributors. They should provide practical guidance for
          // real-world use-cases.
          label: "For Contributors",
          items: [
            {
              label: "Contribution Guidelines",
              link: "contribute/guidelines/",
            },
            {
              label: "Working on documentation",
              link: "contribute/docs/",
            },
            {
              label: "Develop with Nix",
              link: "contribute/nix/",
            },
            {
              label: "Develop with Bazel",
              link: "contribute/bazel/",
            },
            {
              label: "Developing with Cargo",
              link: "contribute/cargo/",
            },
          ],
        },
        {
          // Corresponds to https://diataxis.fr/reference/. Technical
          // descriptions with the intent to be used as consulting material.
          // Mostly autogenerated to stay in sync with the codebase.
          label: "Reference",
          items: [
            {
              label: "Glossary",
              link: "/reference/glossary/",
            },
            {
              label: "Changelog",
              link: "/reference/changelog/",
            },
            {
              label: "Configuration Reference",
              link: "/reference/nativelink-config/",
            },
          ],
        },
        // Navigation.
        {
          label: "leadingNavLinks",
          items: [
            { label: "Docs", link: "/introduction/setup/" },
            { label: "NativeLink Cloud", link: "https://app.nativelink.com/" },
          ],
        },
      ],
    }),
    // Note: Compression should be the last integration.
    playformCompress({
      CSS: {
        lightningcss: { minify: true },
        csso: null,
      },
      HTML: {
        "html-minifier-terser": {
          removeComments: false, // Preserve comments to maintain Qwik's hooks
          collapseWhitespace: false,
          removeAttributeQuotes: false,
          minifyJS: true,
          minifyCSS: true,
        },
      },
      Image: true,
      JavaScript: {
        terser: {
          // Qwik doesn't work with the default settings. Attempt to get as much
          // compression going as possible without breaking anything.
          compress: {
            booleans: true,
            conditionals: true,
            dead_code: true,
            drop_console: false,
            drop_debugger: true,
            evaluate: true,
            hoist_funs: true,
            hoist_vars: true,
            if_return: true,
            join_vars: true,
            keep_fargs: true, // Necessary for function arguments
            keep_fnames: true, // Keep function names for debugging
            loops: true,
            negate_iife: true,
            properties: true,
            reduce_funcs: true,
            reduce_vars: true,
            sequences: true,
            side_effects: true,
            typeofs: false, // Keep typeof
            unused: true,
            warnings: true,
          },
          mangle: {
            // Preserve function names for debugging
            keep_fnames: true,
          },
        },
      },
      SVG: true,
    }),
  ],
});
