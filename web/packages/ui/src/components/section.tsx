import type * as React from "react";
import { cn } from "../lib/cn";

interface SectionProps extends React.HTMLAttributes<HTMLElement> {
  as?: "section" | "div" | "article";
  divider?: boolean;
  width?: "default" | "narrow" | "full";
}

const widthMap: Record<NonNullable<SectionProps["width"]>, string> = {
  default: "max-w-[1200px]",
  narrow: "max-w-[850px]",
  full: "max-w-none",
};

export function Section({
  as: Comp = "section",
  divider = false,
  width = "default",
  className,
  children,
  ...props
}: SectionProps) {
  return (
    <Comp
      className={cn(
        "relative px-6 py-20",
        divider &&
          "after:absolute after:inset-x-[10%] after:bottom-0 after:h-px after:bg-[rgb(var(--nl-color-accent-line))]",
        className,
      )}
      {...props}
    >
      <div className={cn("mx-auto w-full", widthMap[width])}>{children}</div>
    </Comp>
  );
}
