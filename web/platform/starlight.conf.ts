import starlightUtils from "@lorenzo_lewis/starlight-utils";

const docsRoot = "/docs";

export const starlightConfig = {
  title: "NativeLink Docs",
  components: {
    PageFrame: "/src/components/starlight/PageFrame.astro",
  },
  disable404Route: false,
  logo: {
    light: "/src/assets/logo-light.svg",
    dark: "/src/assets/logo-dark.svg",
    replacesTitle: true,
  },
  social: [
    {
      icon: "github",
      label: "GitHub",
      href: "https://github.com/TraceMachina/nativelink",
    },
    {
      icon: "slack",
      label: "Slack",
      href: "https://join.slack.com/t/nativelink/shared_invite/zt-281qk1ho0-krT7HfTUIYfQMdwflRuq7A",
    },
  ],
  customCss: [
    "/styles/tailwind.css",
    "/styles/landing.css",
    "/styles/custom.css",
  ],
  plugins: [
    starlightUtils({
      navLinks: {
        leading: { useSidebarLabelled: "leadingNavLinks" },
      },
    }),
  ],
  routeMiddleware: "./src/routeData.ts",
  sidebar: [
    // The documentation structure follows the Diátaxis framework.
    // See https://diataxis.fr/ for details.
    {
      label: "Getting Started",
      collapsed: true,
      items: [
        {
          label: "Introduction",
          link: `${docsRoot}/introduction/setup`,
        },
        {
          label: "NativeLink On-Prem",
          link: `${docsRoot}/introduction/on-prem`,
        },
        {
          label: "Other Build Systems",
          link: `${docsRoot}/introduction/non-bre`,
        },
      ],
    },
    {
      // Corresponds to https://diataxis.fr/tutorials/. Learning-oriented
      // content without elaborate explanations. Tutorials should have a
      // clear goal and a straightforward "follow-these-commands" structure.
      label: "NativeLink Cloud",
      collapsed: true,
      items: [
        {
          label: "Bazel",
          link: `${docsRoot}/nativelink-cloud/bazel`,
        },
        {
          label: "Reclient",
          link: `${docsRoot}/nativelink-cloud/reclient`,
        },
        {
          label: "Pants",
          link: `${docsRoot}/nativelink-cloud/pants`,
        },
        {
          label: "Remote Build Execution",
          link: `${docsRoot}/nativelink-cloud/rbe`,
        },
        {
          label: "API Keys in CI",
          link: `${docsRoot}/nativelink-cloud/api-key`,
        },
        {
          label: "Nix flake module",
          link: `${docsRoot}/nativelink-cloud/nix`,
        },
      ],
    },
    {
      label: "Testing Remote Execution",
      collapsed: true,
      items: [
        {
          label: "Classic RBE Examples",
          link: `${docsRoot}/rbe/remote-execution-examples`,
        },
        {
          label: "Nix templates",
          link: `${docsRoot}/rbe/nix-templates`,
        },
      ],
    },
    {
      // Corresponds to https://diataxis.fr/how-to-guides/. Guides don't
      // need to be "complete". They should provide practical guidance for
      // real-world use-cases.
      label: "Configuring NativeLink",
      collapsed: true,
      items: [
        {
          label: "Configuration Introduction",
          link: `${docsRoot}/config/configuration-intro`,
        },
        {
          label: "Basic Configurations",
          link: `${docsRoot}/config/basic-configs`,
        },
        {
          label: "Production Configurations",
          link: `${docsRoot}/config/production-config`,
        },
      ],
    },
    {
      // Corresponds to https://diataxis.fr/how-to-guides/. Guides don't
      // need to be "complete". They should provide practical guidance for
      // real-world use-cases.
      label: "On-Prem Examples",
      collapsed: true,
      items: [
        {
          label: "On-Prem Overview",
          link: `${docsRoot}/deployment-examples/on-prem-overview`,
        },
        {
          label: "Kubernetes",
          link: `${docsRoot}/deployment-examples/kubernetes`,
        },
        {
          label: "Chromium",
          link: `${docsRoot}/deployment-examples/chromium`,
        },
      ],
    },
    {
      // Corresponds to https://diataxis.fr/explanation/. Information on
      // internal functionality and design concepts. Explanations should
      // explain design decisions, constraints, etc.
      label: "Understanding NativeLink",
      collapsed: true,
      items: [
        {
          label: "Architecture",
          link: `${docsRoot}/explanations/architecture`,
        },
        {
          label: "History",
          link: `${docsRoot}/explanations/history`,
        },
        {
          label: "Local Remote Execution",
          link: `${docsRoot}/explanations/lre`,
        },
      ],
    },
    {
      // Corresponds to https://diataxis.fr/explanation/. Addresses
      // common questions and confusions about esoteric tooling and
      // concepts. It aims to help new users feel more at ease and
      label: "FAQ",
      collapsed: true,
      items: [
        {
          label: "Is NativeLink Free?",
          link: `${docsRoot}/faq/cost`,
        },
        {
          label: "What is Remote Caching?",
          link: `${docsRoot}/faq/caching`,
        },
        {
          label: "What is Remote Execution?",
          link: `${docsRoot}/faq/remote-execution`,
        },
        {
          label: "What is LRE?",
          link: `${docsRoot}/faq/lre`,
        },
        {
          label: "What are Toolchains?",
          link: `${docsRoot}/faq/toolchains`,
        },
        {
          label: "How do I make my Bazel setup hermetic?",
          link: `${docsRoot}/faq/hermeticity`,
        },
        {
          label: "What is Nix?",
          link: `${docsRoot}/faq/nix`,
        },
        {
          label: "Why Rust?",
          link: `${docsRoot}/faq/rust`,
        },
      ],
    },
    {
      // Corresponds to https://diataxis.fr/how-to-guides/. Guides for
      // contributors. They should provide practical guidance for
      // real-world use-cases.
      label: "For Contributors",
      collapsed: true,
      items: [
        {
          label: "Contribution Guidelines",
          link: `${docsRoot}/contribute/guidelines`,
        },
        {
          label: "Working on documentation",
          link: `${docsRoot}/contribute/docs`,
        },
        {
          label: "Working on the native CLI",
          link: `${docsRoot}/contribute/native-cli`,
        },
        {
          label: "Develop with Nix",
          link: `${docsRoot}/contribute/nix`,
        },
        {
          label: "Develop with Bazel",
          link: `${docsRoot}/contribute/bazel`,
        },
        {
          label: "Developing with Cargo",
          link: `${docsRoot}/contribute/cargo`,
        },
      ],
    },
    {
      // Corresponds to https://diataxis.fr/reference/. Technical
      // descriptions with the intent to be used as consulting material.
      // Mostly autogenerated to stay in sync with the codebase.
      label: "Reference",
      collapsed: true,
      items: [
        {
          label: "Glossary",
          link: `${docsRoot}/reference/glossary`,
        },
        {
          label: "Changelog",
          link: `${docsRoot}/reference/changelog`,
        },
        {
          label: "Configuration Reference",
          link: `${docsRoot}/reference/nativelink-config`,
        },
      ],
    },
    // Navigation.
    {
      label: "leadingNavLinks",
      items: [
        { label: "Docs", link: `${docsRoot}/introduction/setup` },
        { label: "NativeLink Cloud", link: "https://app.nativelink.com/" },
        {
          label: "Coverage",
          link: "https://tracemachina.github.io/nativelink",
        },
      ],
    },
  ],
};
