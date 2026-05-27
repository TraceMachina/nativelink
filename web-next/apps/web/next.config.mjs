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
  // Marketing serves the apex; the docs app lives at /docs/* and is a
  // separate Vercel deployment. We proxy here so /docs URLs stay on the
  // marketing origin (SEO, cookies, one set of analytics).
  //
  //   Production: target is process.env.DOCS_URL (set on Vercel).
  //   Development: target is process.env.DOCS_DEV_URL or localhost:3001.
  async rewrites() {
    const target =
      process.env.NODE_ENV === "production"
        ? process.env.DOCS_URL
        : (process.env.DOCS_DEV_URL ?? "http://localhost:3001");

    // If we're in production and DOCS_URL isn't set, skip the rewrite
    // entirely — better to 404 cleanly than to proxy to nothing.
    if (!target) return [];

    return [
      { source: "/docs", destination: `${target}/docs` },
      { source: "/docs/:path*", destination: `${target}/docs/:path*` },
    ];
  },
};

export default nextConfig;
