/** Normalize a path or href to a comparable pathname (no query, hash, or trailing slash). */
export function normalizeNavPath(hrefOrPath: string): string {
  const raw = hrefOrPath.trim();
  if (
    !raw ||
    raw.startsWith("http://") ||
    raw.startsWith("https://") ||
    raw.startsWith("mailto:")
  ) {
    return raw;
  }

  const query = raw.indexOf("?");
  const hash = raw.indexOf("#");
  let end = raw.length;
  if (query !== -1) {
    end = Math.min(end, query);
  }
  if (hash !== -1) {
    end = Math.min(end, hash);
  }
  let pathOnly = raw.slice(0, end);
  if (pathOnly === "/" || pathOnly === "") {
    return "/";
  }
  while (pathOnly.length > 1 && pathOnly.endsWith("/")) {
    pathOnly = pathOnly.slice(0, -1);
  }
  return pathOnly || "/";
}

/**
 * Whether a nav `href` represents the current route.
 *
 * Home (`/`) is exact-only. Other internal paths match themselves and nested
 * routes (`/resources` on `/resources/blog/slug`). External URLs never match.
 */
export function isCurrentNavHref(href: string, currentPath: string | undefined): boolean {
  if (!currentPath) {
    return false;
  }

  const target = normalizeNavPath(href);
  if (
    target.startsWith("http://") ||
    target.startsWith("https://") ||
    target.startsWith("mailto:")
  ) {
    return false;
  }

  const path = normalizeNavPath(currentPath);
  if (target === "/") {
    return path === "/";
  }
  return path === target || path.startsWith(`${target}/`);
}
