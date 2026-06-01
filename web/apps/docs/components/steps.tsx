import { cn } from "@nativelink/ui";
import type * as React from "react";

/**
 * Numbered tutorial list rendered with a vertical rail and round badges.
 *
 * Layout math (so future-me can re-tune it without re-deriving):
 *   ol has border-l-2 + pl-10 (40px content padding). Its border sits at
 *   left:0..2px in the ol's outer box. li.left (content start) is at
 *   2 + 40 = 42px from the ol's outer left edge.
 *   Each li::before is a 32x32 (w-8 h-8) badge, positioned absolute.
 *   To center the badge on the rail (rail-center ≈ 1px):
 *     badge.left from ol = 1 - 16 = -15px
 *     badge.left from li = -15 - 42 = -57px
 *   Hence the [&>li]:before:left-[-57px] offset below.
 */
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
        // Strip default list markers ONLY from the direct children of this
        // ol — nested ul/ol inside a step still get their disc/decimal
        // bullets back (list-style is inherited; we reset it explicitly).
        "[&>li]:list-none",
        "[&_ul]:list-disc [&_ul]:pl-6 [&_ol]:list-decimal [&_ol]:pl-6",
        // Vertical rail + content padding + step counter.
        "my-8 ml-3 border-l-2 border-border pl-10 [counter-reset:step]",
        // Per-step spacing.
        "[&>li]:relative [&>li]:mb-10 [&>li]:last:mb-0 [&>li]:[counter-increment:step]",
        // Number badge — centered on the rail, sized to read at body font.
        "[&>li]:before:absolute [&>li]:before:left-[-57px] [&>li]:before:top-0",
        "[&>li]:before:flex [&>li]:before:h-8 [&>li]:before:w-8",
        "[&>li]:before:items-center [&>li]:before:justify-center",
        "[&>li]:before:rounded-full [&>li]:before:border-2 [&>li]:before:border-border",
        "[&>li]:before:bg-background [&>li]:before:content-[counter(step)]",
        "[&>li]:before:font-mono [&>li]:before:text-sm [&>li]:before:font-semibold",
        "[&>li]:before:text-brand",
        // Tighter inner rhythm — sub-bullets hug the lead sentence.
        "[&>li_ul]:my-2 [&>li_ol]:my-2 [&>li_p]:my-2",
        className,
      )}
    >
      {children}
    </ol>
  );
}
