import { SOURCE_REF, sourceUrl } from "@/lib/source-ref";
import { cn } from "@nativelink/ui";

interface SourceLinkProps {
  /** Repo-relative path, for example `nativelink-config/src/stores.rs`. */
  file: string;
  /** Symbol to name in the link text, for example `FastSlowSpec`. */
  symbol?: string;
  className?: string;
}

/**
 * A doc-to-source permalink.
 *
 * Every reference entry and every claim about runtime behaviour should carry
 * one, so a reader can check the prose against the code that implements it.
 * The `data-` attributes make the link machine-readable: an agent reading the
 * page can follow the same path to the source that a human can.
 */
export function SourceLink({ file, symbol, className }: SourceLinkProps) {
  const label = symbol ?? file.split("/").pop() ?? file;
  return (
    <a
      href={sourceUrl(file)}
      target="_blank"
      rel="noreferrer"
      data-source-link={file}
      data-source-ref={SOURCE_REF}
      title={`${file} at ${SOURCE_REF}`}
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
