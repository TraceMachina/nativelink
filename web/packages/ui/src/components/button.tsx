import { Slot } from "@radix-ui/react-slot";
import { type VariantProps, cva } from "class-variance-authority";
import * as React from "react";
import { cn } from "../lib/cn";

const buttonVariants = cva(
  "inline-flex cursor-pointer items-center justify-center gap-2 whitespace-nowrap font-medium " +
    "transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60 focus-visible:ring-offset-2 focus-visible:ring-offset-background " +
    "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 min-h-[44px] " +
    "tracking-[-0.005em]",
  {
    variants: {
      variant: {
        primary:
          "bg-brand text-brand-foreground hover:bg-brand-strong " +
          "shadow-[0_1px_0_0_rgb(0_0_0_/_0.05),_0_8px_24px_-12px_rgb(var(--nl-color-brand)/0.55)]",
        solid:
          "bg-foreground text-background hover:bg-accent-hover " +
          "shadow-[0_1px_0_0_rgb(255_255_255_/_0.1)_inset]",
        outline:
          "border border-border bg-surface text-foreground hover:border-foreground hover:bg-foreground hover:text-background",
        ghost:
          "bg-transparent text-foreground hover:bg-foreground/5",
        link:
          "bg-transparent text-brand underline-offset-4 hover:underline px-0 min-h-0 shadow-none",
      },
      size: {
        sm: "h-10 rounded-[6px] px-4 text-sm",
        md: "h-11 rounded-[8px] px-5 text-[15px]",
        lg: "h-12 rounded-[10px] px-7 text-base",
      },
    },
    defaultVariants: {
      variant: "primary",
      size: "md",
    },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp
        ref={ref}
        className={cn(buttonVariants({ variant, size, className }))}
        {...props}
      />
    );
  },
);
Button.displayName = "Button";

export { Button, buttonVariants };
