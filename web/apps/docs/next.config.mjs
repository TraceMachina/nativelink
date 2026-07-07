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
    ];
  },
};

export default withMDX(nextConfig);
