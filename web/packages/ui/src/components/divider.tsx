import type * as React from "react";
import { cn } from "../lib/cn";

interface DividerProps extends React.HTMLAttributes<HTMLHRElement> {
  inset?: boolean;
  tone?: "default" | "brand";
}

export function Divider({ inset = false, tone = "default", className, ...props }: DividerProps) {
  return (
    <hr
      className={cn(
        "h-px border-0",
        tone === "brand" ? "bg-brand/30" : "bg-border",
        inset ? "mx-[10%]" : "w-full",
        className,
      )}
      {...props}
    />
  );
}
