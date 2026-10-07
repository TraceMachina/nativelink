import { cn } from "@nativelink/ui";
import type { ReactNode } from "react";

interface PrerequisitesProps {
  /**
   * What the page assumes the reader already has, written as the state they
   * are in rather than the page they read: "a cache serving hits", not "the
   * quickstart". Link to the page that gets them there when there is one.
   */
  children: ReactNode;
  className?: string;
}

/**
 * States what a page assumes before its first step.
 *
 * Readers arrive mid-corpus from search and from agent caches, not only from
 * the page before. A page in an ordered section therefore has to say what it
 * assumes. Without that, a reader who lands on remote execution with no
 * working cache follows correct instructions to a broken result and blames
 * the product.
 */
export function Prerequisites({ children, className }: PrerequisitesProps) {
  return (
    <div
      data-prerequisites
      className={cn(
        "my-6 rounded-xl border border-border bg-surface-elevated px-5 py-3",
        className,
      )}
    >
      <p className="my-0 font-mono text-xs font-semibold uppercase tracking-[0.12em] text-brand">
        Before you start
      </p>
      <div className="mt-1 text-sm text-muted-foreground [&>p]:my-0">{children}</div>
    </div>
  );
}
