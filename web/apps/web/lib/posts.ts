import fs from "node:fs";
import path from "node:path";

export interface Post {
  slug: string;
  title: string;
  tags: string[];
  image?: string;
  pubDate: string;
  readTime?: string;
  excerpt: string;
  body: string;
}

const POSTS_DIR = path.join(process.cwd(), "content/posts");

// The restored posts (recovered from the pre-redesign Astro site) all share
// the same flat frontmatter shape, so a full YAML parser isn't needed:
//   title: "..."  tags: [".."]  image: url  slug: str  pubDate: date  readTime: str
function parseFrontmatter(raw: string): {
  data: Record<string, string | string[]>;
  body: string;
} {
  const match = raw.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?/);
  if (!match || match[1] === undefined) return { data: {}, body: raw };
  const data: Record<string, string | string[]> = {};
  for (const line of match[1].split(/\r?\n/)) {
    const kv = line.match(/^([A-Za-z]+):\s*(.*)$/);
    const key = kv?.[1];
    if (!kv || key === undefined || kv[2] === undefined) continue;
    const value = kv[2].trim();
    if (value.startsWith("[")) {
      data[key] = value
        .replace(/^\[|\]$/g, "")
        .split(",")
        .map((v) => v.trim().replace(/^["']|["']$/g, ""))
        .filter(Boolean);
    } else {
      data[key] = value.replace(/^["']|["']$/g, "");
    }
  }
  return { data, body: raw.slice(match[0].length) };
}

function deriveExcerpt(body: string): string {
  for (const block of body.split(/\r?\n\r?\n/)) {
    const text = block
      .replace(/^#+\s.*$/gm, "")
      .replace(/^-{3,}\s*$/gm, "")
      .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
      .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/[*_`>]/g, "")
      .replace(/<[^>]+>/g, "")
      .replace(/\s+/g, " ")
      .trim();
    if (text.length > 60) {
      return text.length > 200 ? `${text.slice(0, 197).trimEnd()}…` : text;
    }
  }
  return "";
}

export function getAllPosts(): Post[] {
  return fs
    .readdirSync(POSTS_DIR)
    .filter((f) => /\.(md|mdx)$/.test(f))
    .map((file) => {
      const raw = fs.readFileSync(path.join(POSTS_DIR, file), "utf8");
      const { data, body } = parseFrontmatter(raw);
      const slug =
        typeof data.slug === "string" && data.slug ? data.slug : file.replace(/\.(md|mdx)$/, "");
      return {
        slug,
        title: typeof data.title === "string" ? data.title : slug,
        tags: Array.isArray(data.tags) ? data.tags : [],
        image: typeof data.image === "string" ? data.image : undefined,
        pubDate: typeof data.pubDate === "string" ? data.pubDate : "",
        readTime: typeof data.readTime === "string" ? data.readTime : undefined,
        excerpt: deriveExcerpt(body),
        body,
      };
    })
    .sort((a, b) => b.pubDate.localeCompare(a.pubDate));
}

export function getPost(slug: string): Post | undefined {
  return getAllPosts().find((p) => p.slug === slug);
}

export function formatPostDate(pubDate: string): string {
  const date = new Date(`${pubDate}T00:00:00Z`);
  if (Number.isNaN(date.getTime())) return pubDate;
  return date.toLocaleDateString("en-US", {
    year: "numeric",
    month: "long",
    day: "numeric",
    timeZone: "UTC",
  });
}
