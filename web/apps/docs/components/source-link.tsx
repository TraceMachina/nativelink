import { SOURCE_REF, sourceUrl } from "@/lib/source-ref";
import { cn } from "@nativelink/ui";

interface SourceLinkProps {
  /** Repo-relative path, for example `nativelink-config/src/stores.rs`. */
  file: string;
  /** Symbol to name in the link text, for example `FastSlowSpec`. */
  symbol?: string;
  /**
   * Set when `file` names a directory rather than a file, so the link points
   * at GitHub's tree view. Directory basenames repeat across the repo
   * (`examples/`, `metrics/`), so pass `symbol` too when the last path segment
   * alone would not say which one.
   */
  dir?: boolean;
  className?: string;
}

/**
 * A doc-to-source permalink.
 *
 * Every reference entry and every claim about runtime behaviour should carry
 * one, so a reader can check the prose against the code that implements it.
 * The `data-` attributes make the link machine-readable: an agent reading the
 * page can follow the same path to the source that a human can.
 *
 * Prefer this over a hand-written GitHub URL even for a link that is only
 * pointing at an example config. A hand-written URL names a ref inline, which
 * means it is pinned to `main` (drifts silently) or to a tag that nothing
 * bumps when `SOURCE_REF` moves.
 */
export function SourceLink({ file, symbol, dir, className }: SourceLinkProps) {
  const path = file.replace(/^\/+|\/+$/g, "");
  const base = path.split("/").pop() ?? path;
  const label = symbol ?? (dir ? `${base}/` : base);
  return (
    <a
      href={sourceUrl(path, dir ? "tree" : "blob")}
      target="_blank"
      rel="noreferrer"
      data-source-link={path}
      data-source-ref={SOURCE_REF}
      title={`${path} at ${SOURCE_REF}`}
      className={cn(
        "inline-flex items-center gap-1 rounded-md border border-border",
        "bg-surface-elevated px-1.5 py-0.5 align-middle font-mono text-xs",
        "text-muted-foreground no-underline transition-colors",
        "hover:border-brand/50 hover:text-brand",
        className,
      )}
    >
      <svg
        width="12"
        height="12"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <path d="m16 18 6-6-6-6M8 6l-6 6 6 6" />
      </svg>
      {label}
    </a>
  );
}
