import { createMDX } from "fumadocs-mdx/next";

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  transpilePackages: ["@nativelink/ui"],
  // Preserve links to pages that moved during the docs migration so old,
  // already-published URLs keep resolving instead of 404ing.
  async redirects() {
    return [
      {
        source: "/config/production-config",
        destination: "/operate/production-config",
        permanent: true,
      },
      {
        source: "/getting-started/other-build-systems/reclient",
        destination: "/getting-started/connect-your-build/siso",
        permanent: true,
      },
      // The deployment/ group was dissolved: each page moved to the reader
      // moment it belongs to (a storage recipe, an execution capability, an
      // operations task, a build-tool integration) rather than sitting on a
      // shelf of artifacts.
      ...Object.entries({
        "/deployment/chromium": "/getting-started/connect-your-build/chromium",
        // The Kubernetes page was dropped: the chart it described is an
        // enterprise deliverable, not something this repository ships.
        "/deployment/kubernetes": "/operate",
        "/deployment/metrics": "/operate/observability",
        "/deployment/oci-object-storage": "/how-to/stores/oci-object-storage",
        "/deployment/on-prem-overview": "/operate/deploy-bare-metal",
        "/deployment/persistent-workers": "/remote-execution/persistent-workers",
        // getting-started pages renamed to say what the reader gets rather
        // than what the page is about.
        "/getting-started/on-prem": "/getting-started/shared-cache",
        "/getting-started/other-build-systems":
          "/getting-started/connect-your-build",
        "/getting-started/other-build-systems/:slug*":
          "/getting-started/connect-your-build/:slug*",
        "/getting-started/setup": "/getting-started/quickstart",
      }).map(([source, destination]) => ({
        source,
        destination,
        permanent: true,
      })),
      // The rbe/ group was dissolved into remote-execution/. It had
      // grouped pages by "things that test RBE" rather than by where the
      // reader is: local-remote-execution was really two pages (a toolchain
      // how-to and an explanation), and examples + nix-templates were the
      // same reader moment: "give me something I can copy".
      ...Object.entries({
        "/rbe": "/remote-execution",
        "/rbe/local-testing": "/remote-execution/local-testing",
        "/rbe/local-remote-execution":
          "/remote-execution/toolchains-and-hermeticity",
        "/rbe/examples": "/remote-execution/examples-and-templates",
        "/rbe/nix-templates": "/remote-execution/examples-and-templates",
        "/rbe/:slug*": "/remote-execution/:slug*",
      }).map(([source, destination]) => ({
        source,
        destination,
        permanent: true,
      })),
      // configuration/ was re-scoped to teaching the config model
      // rather than hosting a pile of config-adjacent pages. `intro` and
      // `basic` became the section index and stores.mdx; wire compression and
      // chunking are task recipes, not part of the model; production
      // configuration is an operations concern.
      // Note: enumerated rather than wildcarded, because redirects are matched
      // before filesystem routes and a /configuration/:slug* rule would
      // shadow the real pages in this section.
      ...Object.entries({
        "/configuration/intro": "/configuration",
        "/configuration/basic": "/configuration/stores",
        "/configuration/production": "/operate/production-config",
        "/configuration/compression": "/how-to/stores/compression",
        "/configuration/chunking": "/how-to/stores/chunking-and-dedup",
      }).map(([source, destination]) => ({
        source,
        destination,
        permanent: true,
      })),
      // The standalone /faq section was dissolved into per-page FAQ
      // sections; keep its published URLs resolving to the new homes.
      ...Object.entries({
        architecture: "/explanations/architecture#faq",
        caching: "/getting-started/quickstart#faq",
        clients: "/getting-started/connect-your-build#faq",
        configuration: "/configuration/config-file#faq",
        contributing: "/contribute/guidelines#faq",
        cost: "/getting-started/shared-cache#faq",
        deployment: "/operate/deploy-bare-metal#faq",
        hermeticity: "/remote-execution/toolchains-and-hermeticity#faq",
        lre: "/explanations/lre#faq",
        nix: "/contribute/nix#faq",
        observability: "/operate/observability#faq",
        "remote-execution": "/remote-execution/from-cache-to-execution#faq",
        rust: "/explanations/history#faq",
        stores: "/reference/nativelink-config/store-overview#faq",
        toolchains: "/remote-execution/toolchains-and-hermeticity#faq",
        troubleshooting: "/getting-started/quickstart#faq",
      }).map(([slug, destination]) => ({
        source: `/faq/${slug}`,
        destination,
        permanent: true,
      })),
    ];
  },
};

export default withMDX(nextConfig);
