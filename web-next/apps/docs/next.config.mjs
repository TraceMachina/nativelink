import { createMDX } from "fumadocs-mdx/next";

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  // Docs ship under /docs on the marketing domain. basePath makes every
  // internal route automatically prefixed.
  basePath: "/docs",
  transpilePackages: ["@nativelink/ui"],
};

export default withMDX(nextConfig);
