import { type VariantProps, cva } from "class-variance-authority";
import type * as React from "react";
import { cn } from "../lib/cn";

const badgeVariants = cva(
  "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 font-mono text-[11px] uppercase tracking-[0.12em] leading-none",
  {
    variants: {
      variant: {
        default: "border-border bg-surface text-muted-foreground",
        brand: "border-brand/25 bg-brand-soft text-brand-strong",
        solid: "border-foreground bg-foreground text-background",
        outline: "border-border-strong bg-transparent text-foreground",
        success: "border-success/30 bg-success-soft text-success",
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
