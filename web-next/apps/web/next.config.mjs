/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  transpilePackages: ["@nativelink/ui"],
  images: {
    remotePatterns: [
      { protocol: "https", hostname: "avatars.githubusercontent.com" },
      { protocol: "https", hostname: "github.com" },
    ],
  },
  // In development, transparently proxy /docs/* to the docs Next.js app
  // running on port 3001. In production both apps are deployed separately
  // and the host (Vercel rewrite, Cloudflare worker, etc.) handles routing.
  async rewrites() {
    if (process.env.NODE_ENV === "production") return [];
    const docsTarget = process.env.DOCS_DEV_URL ?? "http://localhost:3001";
    return [
      { source: "/docs", destination: `${docsTarget}/docs` },
      { source: "/docs/:path*", destination: `${docsTarget}/docs/:path*` },
    ];
  },
};

export default nextConfig;
