/**
 * Where doc-to-source links point.
 *
 * Pinned to a release tag, never to `main`. A permalink that drifts is worse
 * than no link at all: a reader who follows it lands on code that no longer
 * matches the prose around it, and has no way to tell. Bump this in the same
 * change that regenerates the config reference for a new release.
 */
export const SOURCE_REF = "v1.6.5";

export const REPO_URL = "https://github.com/TraceMachina/nativelink";

/**
 * Permalink to a repo-relative path at the pinned ref.
 *
 * GitHub serves files under `blob/` and directories under `tree/`. It does
 * redirect between the two, but a link that only works because of a redirect
 * is one platform change away from a 404, so callers say which they mean.
 */
export function sourceUrl(file: string, kind: "blob" | "tree" = "blob"): string {
  return `${REPO_URL}/${kind}/${SOURCE_REF}/${file.replace(/^\/+/, "")}`;
}
