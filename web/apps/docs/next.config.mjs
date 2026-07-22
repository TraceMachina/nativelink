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
        destination: "/configuration/production",
        permanent: true,
      },
      {
        source: "/getting-started/other-build-systems/reclient",
        destination: "/getting-started/other-build-systems/siso",
        permanent: true,
      },
      // The standalone /faq section was dissolved into per-page FAQ
      // sections; keep its published URLs resolving to the new homes.
      ...Object.entries({
        architecture: "/explanations/architecture#faq",
        caching: "/getting-started/setup#faq",
        clients: "/getting-started/other-build-systems#faq",
        configuration: "/configuration/intro#faq",
        contributing: "/contribute/guidelines#faq",
        cost: "/getting-started/on-prem#faq",
        deployment: "/deployment/on-prem-overview#faq",
        hermeticity: "/explanations/lre#faq",
        lre: "/explanations/lre#faq",
        nix: "/contribute/nix#faq",
        observability: "/deployment/metrics#faq",
        "remote-execution": "/rbe/examples#faq",
        rust: "/explanations/history#faq",
        stores: "/reference/nativelink-config/store-overview#faq",
        toolchains: "/rbe/examples#faq",
        troubleshooting: "/getting-started/setup#faq",
      }).map(([slug, destination]) => ({
        source: `/faq/${slug}`,
        destination,
        permanent: true,
      })),
    ];
  },
};

export default withMDX(nextConfig);
