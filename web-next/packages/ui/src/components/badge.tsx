import { type VariantProps, cva } from "class-variance-authority";
import * as React from "react";
import { cn } from "../lib/cn";

const badgeVariants = cva(
  "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 font-mono text-xs leading-none",
  {
    variants: {
      variant: {
        default: "border-border bg-surface/60 text-muted-foreground",
        solid: "border-foreground bg-foreground text-background",
        outline: "border-foreground bg-transparent text-foreground",
        accent:
          "border-[rgb(var(--nl-color-accent-line))] bg-transparent text-foreground",
      },
    },
    defaultVariants: { variant: "default" },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

export function Badge({ className, variant, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ variant, className }))} {...props} />;
}

export { badgeVariants };
