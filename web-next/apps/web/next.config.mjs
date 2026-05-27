/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  transpilePackages: ["@nativelink/ui"],
  experimental: {
    // Enable typed routes once we add real routing.
    // typedRoutes: true,
  },
};

export default nextConfig;
