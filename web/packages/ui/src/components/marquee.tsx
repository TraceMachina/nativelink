import type * as React from "react";
import { cn } from "../lib/cn";

interface MarqueeProps extends React.HTMLAttributes<HTMLDivElement> {
  speed?: number; // seconds per loop
  reverse?: boolean;
  pauseOnHover?: boolean;
  children: React.ReactNode;
}

export function Marquee({
  speed = 30,
  reverse = false,
  pauseOnHover = true,
  className,
  children,
  ...props
}: MarqueeProps) {
  return (
    <div
      className={cn(
        "group relative flex w-full overflow-hidden",
        "[mask-image:linear-gradient(to_right,transparent,black_8%,black_92%,transparent)]",
        className,
      )}
      {...props}
    >
      <div
        className={cn(
          "flex shrink-0 items-center gap-12 whitespace-nowrap",
          "motion-safe:animate-[marquee_var(--marquee-duration)_linear_infinite]",
          pauseOnHover && "group-hover:[animation-play-state:paused]",
        )}
        style={
          {
            ["--marquee-duration" as string]: `${speed}s`,
            animationDirection: reverse ? "reverse" : "normal",
          } as React.CSSProperties
        }
      >
        {children}
        {/* Duplicate for seamless loop */}
        <div aria-hidden="true" className="flex shrink-0 items-center gap-12">
          {children}
        </div>
      </div>
    </div>
  );
}
