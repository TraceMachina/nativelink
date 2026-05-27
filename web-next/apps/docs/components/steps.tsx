import { cn } from "@nativelink/ui";
import type * as React from "react";

export function Steps({
  className,
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <ol
      className={cn(
        // Suppress the default 1./2. markers — we render our own badge
        // via the [&>li]:before pseudo-element below.
        "list-none [&_li::marker]:content-none",
        // Vertical rail on the left, counter-reset for numbering.
        "my-8 ml-3 border-l-2 border-border pl-8 [counter-reset:step]",
        // Per-step spacing + numbering badge.
        "[&>li]:relative [&>li]:mb-10 [&>li]:last:mb-0",
        "[&>li]:[counter-increment:step]",
        // Number badge — circle, sits on the rail.
        "[&>li]:before:absolute [&>li]:before:left-[-44px] [&>li]:before:top-[-2px]",
        "[&>li]:before:flex [&>li]:before:h-8 [&>li]:before:w-8 [&>li]:before:items-center",
        "[&>li]:before:justify-center [&>li]:before:rounded-full",
        "[&>li]:before:border-2 [&>li]:before:border-border [&>li]:before:bg-background",
        "[&>li]:before:content-[counter(step)]",
        "[&>li]:before:font-mono [&>li]:before:text-sm [&>li]:before:font-semibold",
        "[&>li]:before:text-brand",
        // Nested lists (bullets, sub-steps) inside a step get tighter
        // top margin so they hug the lead paragraph.
        "[&>li_ul]:my-3 [&>li_ol]:my-3",
        "[&>li_p]:my-2",
        className,
      )}
    >
      {children}
    </ol>
  );
}
