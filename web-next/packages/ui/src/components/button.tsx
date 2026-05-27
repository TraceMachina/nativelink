import { Slot } from "@radix-ui/react-slot";
import { type VariantProps, cva } from "class-variance-authority";
import * as React from "react";
import { cn } from "../lib/cn";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap font-mono font-medium " +
    "transition-colors focus-visible:outline-none focus-visible:ring-2 " +
    "focus-visible:ring-foreground/40 disabled:pointer-events-none disabled:opacity-50 " +
    "min-h-[44px]",
  {
    variants: {
      variant: {
        primary:
          "bg-foreground text-background border-2 border-foreground rounded-md " +
          "hover:bg-[rgb(var(--nl-color-accent-hover))]",
        outline:
          "bg-transparent text-foreground border-2 border-foreground rounded-md " +
          "hover:bg-foreground hover:text-background",
        ghost:
          "bg-transparent text-foreground rounded-md hover:bg-foreground/5",
        link: "bg-transparent text-foreground underline-offset-4 hover:underline px-0",
      },
      size: {
        sm: "h-11 px-4 text-sm",
        md: "h-12 px-6 text-base",
        lg: "h-14 px-10 text-base",
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
