import createMDX from "@next/mdx";

/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  transpilePackages: ["@nativelink/ui"],
  pageExtensions: ["ts", "tsx", "js", "jsx", "md", "mdx"],
  images: {
    remotePatterns: [
      { protocol: "https", hostname: "avatars.githubusercontent.com" },
      { protocol: "https", hostname: "github.com" },
      {
        protocol: "https",
        hostname: "nativelink-cdn.s3.us-east-1.amazonaws.com",
      },
    ],
  },
  // Marketing serves the apex; the docs app lives on docs.nativelink.com.
  // Redirect /docs links there so old URLs and local nav links keep working.
  //
  //   Production: target is process.env.DOCS_URL (set on Vercel).
  //   Development: target is process.env.DOCS_DEV_URL or localhost:3001.
  async redirects() {
    const target =
      process.env.NODE_ENV === "production"
        ? process.env.DOCS_URL
        : (process.env.DOCS_DEV_URL ?? "http://localhost:3001");

    // If we're in production and DOCS_URL isn't set, skip the redirect
    // entirely — better to 404 cleanly than to redirect to nothing.
    if (!target) return [];

    return [
      { source: "/docs", destination: target, permanent: false },
      { source: "/docs/:path*", destination: `${target}/:path*`, permanent: false },
    ];
  },
};

const withMDX = createMDX();

export default withMDX(nextConfig);
