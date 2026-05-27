import { cn } from "@nativelink/ui";
import type * as React from "react";

export function Steps({
  className,
  children,
}: { className?: string; children: React.ReactNode }) {
  return (
    <ol
      className={cn(
        "my-6 ml-2 border-l border-border [counter-reset:step]",
        "[&>li]:relative [&>li]:mb-8 [&>li]:pl-8",
        "[&>li]:[counter-increment:step]",
        "[&>li]:before:absolute [&>li]:before:left-[-13px] [&>li]:before:top-0",
        "[&>li]:before:flex [&>li]:before:h-6 [&>li]:before:w-6 [&>li]:before:items-center",
        "[&>li]:before:justify-center [&>li]:before:rounded-full [&>li]:before:border [&>li]:before:border-border",
        "[&>li]:before:bg-surface [&>li]:before:content-[counter(step)]",
        "[&>li]:before:font-mono [&>li]:before:text-xs [&>li]:before:font-semibold",
        "[&>li]:before:text-brand",
        className,
      )}
    >
      {children}
    </ol>
  );
}
