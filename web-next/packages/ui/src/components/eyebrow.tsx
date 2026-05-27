import * as React from "react";
import { cn } from "../lib/cn";

export function Eyebrow({
  className,
  ...props
}: React.HTMLAttributes<HTMLParagraphElement>) {
  return (
    <p
      className={cn(
        "font-mono text-xs uppercase tracking-[0.18em] text-muted",
        className,
      )}
      {...props}
    />
  );
}
