import * as React from "react";
import { cn } from "../lib/cn";

interface LogoProps extends React.HTMLAttributes<HTMLSpanElement> {
  size?: "sm" | "md" | "lg" | "xl";
  /** Prefix prepended to the asset paths. Pass the consuming app's
   *  Next.js basePath here (e.g. "/docs") — raw <img src> doesn't get
   *  auto-prefixed by Next, unlike next/image. Default is empty. */
  basePath?: string;
}

const sizeMap: Record<NonNullable<LogoProps["size"]>, number> = {
  sm: 20,
  md: 24,
  lg: 32,
  xl: 44,
};

export function Logo({
  size = "md",
  className,
  basePath = "",
  ...props
}: LogoProps) {
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
      <img
        src={`${basePath}/logo-light.svg`}
        alt="NativeLink"
        width={w}
        height={h}
        className="block h-full w-full dark:hidden"
        draggable={false}
      />
      <img
        src={`${basePath}/logo-dark.svg`}
        alt="NativeLink"
        width={w}
        height={h}
        className="hidden h-full w-full dark:block"
        draggable={false}
      />
    </span>
  );
}
