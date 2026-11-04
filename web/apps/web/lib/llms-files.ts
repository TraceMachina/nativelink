// The three llms files the site serves, and their sizes as built.
//
// The sizes are read from the generated files in public/ at build time, so the
// page can never print a number the files have grown past. gen-llms runs before
// `next build` and before `next dev` (see package.json), so the files exist
// whenever a page is rendered; if one is missing the page omits the size.
import { statSync } from "node:fs";
import { join } from "node:path";

export interface LlmsFile {
  name: "llms.txt" | "llms-small.txt" | "llms-full.txt";
  label: string;
  body: string;
  /** When an agent should reach for this one. */
  when: string;
}

export const LLMS_FILES: LlmsFile[] = [
  {
    name: "llms.txt",
    label: "The index",
    body: "Every page with a one-line description, in reading order, following the llms.txt convention.",
    when: "Fetch this first, to decide what to fetch next.",
  },
  {
    name: "llms-small.txt",
    label: "The abridged corpus",
    body: "Every page of this site in prose, plus the documentation with its headings and the sentence that opens each section. No code samples.",
    when: "Fetch this for most questions.",
  },
  {
    name: "llms-full.txt",
    label: "Everything",
    body: "The whole site, every blog post, and the documentation with page bodies verbatim, each delimited by its canonical URL. The three generated reference pages (config, metrics, changelog) are linked rather than inlined.",
    when: "Fetch this only when the small file lacks the answer.",
  },
];

/** Bytes on disk of a generated llms file, or null if it has not been generated. */
export function llmsFileBytes(name: LlmsFile["name"], publicDir = join(process.cwd(), "public")) {
  try {
    return statSync(join(publicDir, name)).size;
  } catch {
    return null;
  }
}

/** "28 KB", "222 KB", "1.1 MB": decimal units, the way a browser reports a download. */
export function formatBytes(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`;
  if (bytes < 1_000_000) return `${Math.round(bytes / 1000)} KB`;
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

/**
 * A rough token count at four bytes per token, the usual estimate for English
 * prose and the one an agent can check against its own tokenizer.
 */
export function approxTokens(bytes: number): number {
  return Math.round(bytes / 4);
}

/** "7k tokens", "55k tokens", "276k tokens"; exact below a thousand. */
export function formatTokens(tokens: number): string {
  if (tokens < 1000) return `${tokens} tokens`;
  return `${Math.round(tokens / 1000)}k tokens`;
}

/** The size line printed on a file card, or null when the file is not on disk. */
export function sizeLine(bytes: number | null): string | null {
  if (bytes === null) return null;
  return `${formatBytes(bytes)} · about ${formatTokens(approxTokens(bytes))}`;
}
