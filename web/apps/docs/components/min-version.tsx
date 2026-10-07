import { cn } from "@nativelink/ui";

interface MinVersionProps {
  /** The first release in which this works, without the leading `v`. */
  v: string;
  className?: string;
}

/**
 * Inline "needs at least this release" badge.
 *
 * Put one next to any field, flag, or command that does not exist in every
 * supported release. Readers land on these pages from search engines and from
 * agent caches, on whatever version they happen to be running, so "which
 * version is this?" has to be answerable on the page itself.
 */
export function MinVersion({ v, className }: MinVersionProps) {
  return (
    <span
      data-min-version={v}
      className={cn(
        "ml-1 inline-flex items-center rounded-full border border-brand/40",
        "bg-brand-soft/40 px-2 py-0.5 align-middle font-mono text-[0.7rem]",
        "font-semibold text-brand",
        className,
      )}
    >
      {v}+
    </span>
  );
}
