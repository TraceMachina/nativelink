import * as React from "react";
import { cn } from "../lib/cn";

interface LogoProps extends React.HTMLAttributes<HTMLSpanElement> {
  size?: "sm" | "md" | "lg" | "xl";
}

const sizeMap: Record<NonNullable<LogoProps["size"]>, number> = {
  sm: 20,
  md: 24,
  lg: 32,
  xl: 44,
};

/**
 * Original NativeLink wordmark — uses the gradient/colored SVGs from the
 * legacy site. Two variants ship: -light for dark backgrounds, -dark for
 * light backgrounds. The dark: utility swaps based on our data-theme attr.
 *
 * Original aspect ratio is roughly 1080 × 177 (≈6.1:1).
 */
export function Logo({ size = "md", className, ...props }: LogoProps) {
  const h = sizeMap[size];
  const w = Math.round(h * 6.105);
  return (
    <span
      aria-label="NativeLink"
      role="img"
      className={cn("inline-flex shrink-0 items-center", className)}
      style={{ height: h, width: w }}
      {...props}
    >
      {/* Light theme — logo file is named after the theme it serves */}
      <img
        src="/logo-light.svg"
        alt="NativeLink"
        width={w}
        height={h}
        className="block h-full w-full dark:hidden"
        draggable={false}
      />
      {/* Dark theme */}
      <img
        src="/logo-dark.svg"
        alt="NativeLink"
        width={w}
        height={h}
        className="hidden h-full w-full dark:block"
        draggable={false}
      />
    </span>
  );
}
