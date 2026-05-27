import * as React from "react";
import { cn } from "../lib/cn";

interface LogoProps extends React.SVGAttributes<SVGSVGElement> {
  size?: "sm" | "md" | "lg" | "xl";
  wordmark?: boolean;
}

const sizeMap: Record<NonNullable<LogoProps["size"]>, { h: number; markW: number; wordmarkW: number }> = {
  sm: { h: 20, markW: 20, wordmarkW: 124 },
  md: { h: 24, markW: 24, wordmarkW: 152 },
  lg: { h: 32, markW: 32, wordmarkW: 198 },
  xl: { h: 44, markW: 44, wordmarkW: 268 },
};

export function Logo({ size = "md", wordmark = true, className, ...props }: LogoProps) {
  const { h, markW, wordmarkW } = sizeMap[size];
  const id = React.useId();
  return (
    <svg
      role="img"
      aria-label="NativeLink"
      width={wordmark ? wordmarkW : markW}
      height={h}
      viewBox={wordmark ? "0 0 152 24" : "0 0 24 24"}
      fill="none"
      className={cn("text-foreground", className)}
      {...props}
    >
      <defs>
        <linearGradient id={`nl-grad-${id}`} x1="2" y1="2" x2="22" y2="22" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stopColor="rgb(var(--nl-color-brand))" />
          <stop offset="100%" stopColor="rgb(var(--nl-color-brand-strong))" />
        </linearGradient>
        <radialGradient id={`nl-glow-${id}`} cx="12" cy="12" r="14" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stopColor="rgb(var(--nl-color-brand))" stopOpacity="0.5" />
          <stop offset="100%" stopColor="rgb(var(--nl-color-brand))" stopOpacity="0" />
        </radialGradient>
      </defs>
      {/* Inner glow halo */}
      <rect x="-4" y="-4" width="32" height="32" fill={`url(#nl-glow-${id})`} />
      {/* Mark: tilted square with chevron-N inside */}
      <rect
        x="0.5"
        y="0.5"
        width="23"
        height="23"
        rx="6"
        fill={`url(#nl-grad-${id})`}
      />
      <rect x="0.5" y="0.5" width="23" height="23" rx="6" fill="none" stroke="rgb(var(--nl-color-brand-foreground) / 0.18)" strokeWidth="1" />
      {/* Chunky N with thick diagonal stroke */}
      <path
        d="M5.6 6.5 H8.3 V13.8 L15.2 6.5 H18.4 V17.5 H15.7 V10.2 L8.8 17.5 H5.6 Z"
        fill="rgb(var(--nl-color-brand-foreground))"
      />
      {/* Accent dot — signals connection / cache hit */}
      <circle cx="20" cy="4" r="1.4" fill="rgb(var(--nl-color-success))" />
      {wordmark && (
        <text
          x="32"
          y="17"
          fontFamily="var(--nl-font-sans)"
          fontSize="15.5"
          fontWeight="600"
          letterSpacing="-0.025em"
          fill="currentColor"
        >
          nativelink
        </text>
      )}
    </svg>
  );
}
