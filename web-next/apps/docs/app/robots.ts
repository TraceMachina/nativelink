import type { MetadataRoute } from "next";

const BASE_URL = "https://nativelink.com";

export default function robots(): MetadataRoute.Robots {
  return {
    rules: [{ userAgent: "*", allow: "/" }],
    sitemap: `${BASE_URL}/docs/sitemap.xml`,
    host: BASE_URL,
  };
}
