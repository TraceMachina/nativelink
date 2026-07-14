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
      // Production configuration is an operations concern, not a
      // config-authoring one: it leans on observability, scaling and
      // hardening, which are its neighbours under operate/.
      {
        source: "/configuration/production",
        destination: "/operate/production-config",
        permanent: true,
      },
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
        "/deployment/kubernetes": "/operate/deploy-kubernetes",
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
      // The standalone /faq section was dissolved into per-page FAQ
      // sections; keep its published URLs resolving to the new homes.
      ...Object.entries({
        architecture: "/explanations/architecture#faq",
        caching: "/getting-started/quickstart#faq",
        clients: "/getting-started/connect-your-build#faq",
        configuration: "/configuration/intro#faq",
        contributing: "/contribute/guidelines#faq",
        cost: "/getting-started/shared-cache#faq",
        deployment: "/operate/deploy-bare-metal#faq",
        hermeticity: "/explanations/lre#faq",
        lre: "/explanations/lre#faq",
        nix: "/contribute/nix#faq",
        observability: "/operate/observability#faq",
        "remote-execution": "/rbe/examples#faq",
        rust: "/explanations/history#faq",
        stores: "/reference/nativelink-config/store-overview#faq",
        toolchains: "/rbe/examples#faq",
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
