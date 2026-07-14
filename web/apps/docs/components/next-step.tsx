import { cn } from "@nativelink/ui";
import type * as React from "react";

interface NextStepProps {
  /** Where the reader goes next. */
  href: string;
  /** The destination's name, as the reader will see it in the sidebar. */
  title: string;
  /**
   * `next` continues along the ladder; `aside` is a useful detour that does
   * not advance the reader's position. Rendering them differently keeps the
   * main path obvious when a page offers both.
   */
  kind?: "next" | "aside";
  /** Why the reader would go there — one sentence, not a description. */
  children: React.ReactNode;
  className?: string;
}

/**
 * The journey handoff.
 *
 * A page that ends without telling the reader where to go next has dumped
 * information on them. This component renders that handoff the same way on
 * every page, so the path through the docs is visible rather than implied.
 */
export function NextStep({
  href,
  title,
  kind = "next",
  children,
  className,
}: NextStepProps) {
  const isNext = kind === "next";
  return (
    <a
      href={href}
      data-next-step={kind}
      className={cn(
        "my-4 flex items-start gap-3 rounded-xl border px-5 py-4 no-underline",
        "transition-colors",
        isNext
          ? "border-brand/40 bg-brand-soft/30 hover:border-brand/70"
          : "border-border bg-surface-elevated hover:border-border-strong",
        className,
      )}
    >
      <svg
        width="18"
        height="18"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className={cn(
          "mt-1 shrink-0",
          isNext ? "text-brand" : "text-muted-foreground",
        )}
        aria-hidden="true"
      >
        {isNext ? (
          <path d="M5 12h14m-6-6 6 6-6 6" />
        ) : (
          <path d="M5 12h14m-6-6 6 6-6 6" opacity="0.55" />
        )}
      </svg>
      <span className="flex-1">
        <span className="block font-mono text-[0.7rem] font-semibold uppercase tracking-[0.12em] text-muted-foreground">
          {isNext ? "Next" : "Sideways"}
        </span>
        <span className="block font-semibold text-foreground">{title}</span>
        <span className="mt-0.5 block text-sm text-muted-foreground">
          {children}
        </span>
      </span>
    </a>
  );
}
