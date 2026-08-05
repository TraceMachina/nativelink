import { source } from "@/lib/source";
import type { MetadataRoute } from "next";

const BASE_URL = "https://docs.nativelink.com";

export default function sitemap(): MetadataRoute.Sitemap {
  const lastModified = new Date();
  return source.getPages().map((page) => ({
    url: `${BASE_URL}${page.url}`,
    lastModified,
    changeFrequency: "weekly" as const,
    priority: page.url === "/" ? 0.9 : 0.6,
  }));
}
