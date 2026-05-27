import * as React from "react";
import { cn } from "../lib/cn";

interface DividerProps extends React.HTMLAttributes<HTMLHRElement> {
  inset?: boolean;
}

export function Divider({ inset = false, className, ...props }: DividerProps) {
  return (
    <hr
      role="separator"
      className={cn(
        "h-px border-0 bg-[rgb(var(--nl-color-accent-line))]",
        inset ? "mx-[10%]" : "w-full",
        className,
      )}
      {...props}
    />
  );
}
