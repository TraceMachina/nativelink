"use client";

import { animate, useInView } from "motion/react";
import * as React from "react";

interface CounterProps {
  to: number;
  from?: number;
  duration?: number;
  decimals?: number;
  prefix?: string;
  suffix?: string;
  className?: string;
  formatter?: (n: number) => string;
}

const defaultFormatter = (n: number, decimals: number) =>
  decimals > 0 ? n.toFixed(decimals) : Math.round(n).toLocaleString();

export function Counter({
  to,
  from = 0,
  duration = 1.6,
  decimals = 0,
  prefix = "",
  suffix = "",
  className,
  formatter,
}: CounterProps) {
  const ref = React.useRef<HTMLSpanElement>(null);
  const inView = useInView(ref, { once: true, margin: "-30% 0px" });
  const [value, setValue] = React.useState(from);

  React.useEffect(() => {
    if (!inView) return;
    const controls = animate(from, to, {
      duration,
      ease: [0.16, 1, 0.3, 1],
      onUpdate: (v) => setValue(v),
    });
    return () => controls.stop();
  }, [inView, from, to, duration]);

  const formatted = formatter ? formatter(value) : defaultFormatter(value, decimals);

  return (
    <span ref={ref} className={className}>
      {prefix}
      {formatted}
      {suffix}
    </span>
  );
}
