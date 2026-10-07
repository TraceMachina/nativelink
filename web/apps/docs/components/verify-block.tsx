import { cn } from "@nativelink/ui";
import type * as React from "react";

interface VerifyBlockProps {
  /** Overrides the default heading when a page needs a more specific one. */
  title?: string;
  children: React.ReactNode;
  className?: string;
}

/**
 * The "you did it right if..." box.
 *
 * Every tutorial step and every how-to ends with one of these. Without it a
 * reader who followed the instructions has no way to distinguish "it worked"
 * from "it appeared to work", which, for a build cache, is the difference
 * between a fast build and a silently cold one.
 *
 * `data-verify-block` is deliberate: it makes the claim machine-detectable, so
 * CI can assert that no tutorial or how-to page ships without one, and an agent
 * reading the page can tell instructions apart from their success criteria.
 */
export function VerifyBlock({
  title = "You did it right if",
  children,
  className,
}: VerifyBlockProps) {
  return (
    <div
      data-verify-block=""
      className={cn(
        "my-6 rounded-xl border border-success/40 bg-success-soft/50 px-5 py-4",
        className,
      )}
    >
      <p className="mb-2 flex items-center gap-2 font-mono text-xs font-semibold uppercase tracking-[0.12em] text-foreground">
        <svg
          width="16"
          height="16"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.25"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="shrink-0 text-success"
          aria-hidden="true"
        >
          <path d="M20 6 9 17l-5-5" />
        </svg>
        {title}
      </p>
      <div className="[&>p]:my-0 [&>p+p]:mt-2 [&>ul]:my-0">{children}</div>
    </div>
  );
}
