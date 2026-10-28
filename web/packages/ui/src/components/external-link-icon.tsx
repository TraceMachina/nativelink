import { cn } from "../lib/cn";

/**
 * Arrow-up-right glyph for a link that leaves the marketing site (the docs,
 * the Enterprise portal, GitHub). Pair it with `leavesSite` from `lib/nav`.
 * Inherits the link's colour; carries its own screen-reader text.
 */
export function ExternalLinkIcon({ className }: { className?: string }) {
  return (
    <>
      <svg
        width="10"
        height="10"
        viewBox="0 0 10 10"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        className={cn("shrink-0 opacity-70", className)}
      >
        <path d="M2.5 7.5 7.5 2.5M3.5 2.5h4v4" />
      </svg>
      <span className="sr-only"> (opens in a new tab)</span>
    </>
  );
}
