import type * as React from "react";
import { cn } from "../lib/cn";

interface EyebrowProps extends React.HTMLAttributes<HTMLParagraphElement> {
  tone?: "brand" | "muted";
}

export function Eyebrow({ tone = "brand", className, ...props }: EyebrowProps) {
  return (
    <p
      className={cn(
        "font-mono text-xs uppercase tracking-[0.18em]",
        tone === "brand" ? "text-brand" : "text-muted",
        className,
      )}
      {...props}
    />
  );
}
