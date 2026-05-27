import { createMDX } from "fumadocs-mdx/next";

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  // Docs ship under /docs. With basePath set, every internal Next.js link
  // and every emitted asset URL (/_next/static/*) is automatically
  // prefixed — which is what makes the marketing dev rewrite work for
  // both pages and assets in one rule.
  basePath: "/docs",
  transpilePackages: ["@nativelink/ui"],
};

export default withMDX(nextConfig);
