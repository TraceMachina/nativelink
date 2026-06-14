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

    const redirects = [];

    // Redirect www to apex domain
    if (process.env.NODE_ENV === "production") {
      redirects.push({
        source: "/:path*",
        has: [{ type: "host", value: "www.nativelink.com" }],
        destination: "https://nativelink.com/:path*",
        permanent: true,
      });
    }

    // If we're in production and DOCS_URL isn't set, skip the docs redirect
    // entirely — better to 404 cleanly than to redirect to nothing.
    if (target) {
      redirects.push(
        { source: "/docs", destination: target, permanent: false },
        { source: "/docs/:path*", destination: `${target}/:path*`, permanent: false }
      );
    }

    return redirects;
  },
};

export default nextConfig;
