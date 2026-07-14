/**
 * Where doc-to-source links point.
 *
 * Pinned to a release tag, never to `main`. A permalink that drifts is worse
 * than no link at all: a reader who follows it lands on code that no longer
 * matches the prose around it, and has no way to tell. Bump this in the same
 * change that regenerates the config reference for a new release.
 */
export const SOURCE_REF = "v1.6.3";

export const REPO_URL = "https://github.com/TraceMachina/nativelink";

/** Permalink to a repo-relative path at the pinned ref. */
export function sourceUrl(file: string): string {
  return `${REPO_URL}/blob/${SOURCE_REF}/${file.replace(/^\/+/, "")}`;
}
