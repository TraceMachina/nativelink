import * as React from "react";
import { cn } from "../lib/cn";

interface LogoProps extends React.SVGAttributes<SVGSVGElement> {
  size?: "sm" | "md" | "lg";
}

const sizeMap: Record<NonNullable<LogoProps["size"]>, { h: number; w: number }> = {
  sm: { h: 18, w: 132 },
  md: { h: 22, w: 162 },
  lg: { h: 28, w: 206 },
};

export function Logo({ size = "md", className, ...props }: LogoProps) {
  const { h, w } = sizeMap[size];
  return (
    <svg
      role="img"
      aria-label="NativeLink"
      width={w}
      height={h}
      viewBox="0 0 162 22"
      fill="currentColor"
      className={cn("text-foreground", className)}
      {...props}
    >
      <rect x="0" y="3" width="16" height="16" rx="2" />
      <rect x="3" y="6" width="10" height="10" rx="1" fill="rgb(var(--nl-color-background))" />
      <text
        x="24"
        y="16"
        fontFamily="var(--nl-font-mono)"
        fontSize="14"
        fontWeight="700"
        letterSpacing="-0.02em"
      >
        nativelink
      </text>
    </svg>
  );
}
