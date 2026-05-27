import * as React from "react";
import { cn } from "../lib/cn";

interface LogoProps extends React.SVGAttributes<SVGSVGElement> {
  size?: "sm" | "md" | "lg";
  wordmark?: boolean;
}

const sizeMap: Record<NonNullable<LogoProps["size"]>, { h: number; w: number; mark: number }> = {
  sm: { h: 20, w: 132, mark: 20 },
  md: { h: 24, w: 158, mark: 24 },
  lg: { h: 32, w: 208, mark: 32 },
};

export function Logo({ size = "md", wordmark = true, className, ...props }: LogoProps) {
  const { h, w, mark } = sizeMap[size];
  return (
    <svg
      role="img"
      aria-label="NativeLink"
      width={wordmark ? w : mark}
      height={h}
      viewBox={wordmark ? "0 0 158 24" : "0 0 24 24"}
      fill="none"
      className={cn("text-foreground", className)}
      {...props}
    >
      <defs>
        <linearGradient id="nl-mark-gradient" x1="0" y1="0" x2="24" y2="24" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stopColor="rgb(var(--nl-color-brand))" />
          <stop offset="100%" stopColor="rgb(var(--nl-color-brand-strong))" />
        </linearGradient>
      </defs>
      <rect x="0" y="2" width="20" height="20" rx="6" fill="url(#nl-mark-gradient)" />
      <path
        d="M5 16V8h2.4l5.2 5.6V8H15v8h-2.4l-5.2-5.6V16H5Z"
        fill="rgb(var(--nl-color-brand-foreground))"
      />
      {wordmark && (
        <text
          x="28"
          y="17"
          fontFamily="var(--nl-font-sans)"
          fontSize="15"
          fontWeight="600"
          letterSpacing="-0.02em"
          fill="currentColor"
        >
          nativelink
        </text>
      )}
    </svg>
  );
}
