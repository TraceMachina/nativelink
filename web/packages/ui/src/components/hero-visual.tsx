"use client";

import * as motion from "motion/react-client";
import { cn } from "../lib/cn";

interface HeroVisualProps {
  className?: string;
}

/**
 * Seven days, two series per day, as the legend promises: wall-time with a
 * warm cache ("cached") against the same builds from scratch ("fresh"), as a
 * percentage of the tallest bar. Cached is always the shorter of the pair;
 * that gap is the chart's whole point.
 */
const days = [
  { label: "Mon", fresh: 62, cached: 24 },
  { label: "Tue", fresh: 70, cached: 22 },
  { label: "Wed", fresh: 84, cached: 27 },
  { label: "Thu", fresh: 78, cached: 19 },
  { label: "Fri", fresh: 100, cached: 31 },
  { label: "Sat", fresh: 46, cached: 12 },
  { label: "Sun", fresh: 38, cached: 9 },
];

export function HeroVisual({ className }: HeroVisualProps) {
  return (
    <div className={cn("relative w-full", className)}>
      {/* Soft glow halo behind the panel */}
      <div className="pointer-events-none absolute -inset-8 -z-10 rounded-[40px] bg-gradient-to-br from-brand/25 via-brand/10 to-brand-strong/20 blur-3xl" />

      {/* Outer panel — the "app window" */}
      <div className="relative overflow-hidden rounded-2xl border border-border bg-surface shadow-[0_30px_80px_-25px_rgb(0_0_0_/_0.25),_0_4px_12px_-4px_rgb(0_0_0_/_0.08)] backdrop-blur-sm">
        {/* Title bar */}
        <div className="flex items-center gap-3 border-b border-border/70 bg-surface-elevated px-4 py-3">
          <div className="flex gap-1.5">
            <span className="h-2.5 w-2.5 rounded-full bg-foreground/20" />
            <span className="h-2.5 w-2.5 rounded-full bg-foreground/20" />
            <span className="h-2.5 w-2.5 rounded-full bg-foreground/20" />
          </div>
          <div className="ml-3 flex items-center gap-2 rounded-md bg-background/70 px-3 py-1 font-mono text-[11px] text-muted-foreground">
            <span className="inline-block h-1.5 w-1.5 rounded-full bg-success" />
            nativelink.cloud / dashboard
          </div>
          <div className="ml-auto flex items-center gap-1">
            <span className="rounded-md bg-foreground/5 px-2 py-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
              live
            </span>
          </div>
        </div>

        {/* Body */}
        <div className="grid grid-cols-[1fr_1.4fr] gap-4 p-5">
          {/* Stats column */}
          <div className="flex flex-col gap-3">
            <div className="rounded-xl border border-border bg-surface-elevated p-4">
              <p className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
                Cache hit rate
              </p>
              <p className="mt-2 text-2xl font-semibold tracking-tight text-foreground">94.7%</p>
              <div className="mt-3 h-1.5 w-full overflow-hidden rounded-full bg-foreground/[0.06]">
                <motion.div
                  initial={{ width: 0 }}
                  whileInView={{ width: "94.7%" }}
                  viewport={{ once: true }}
                  transition={{ duration: 1.2, delay: 0.3, ease: [0.16, 1, 0.3, 1] }}
                  className="h-full rounded-full bg-gradient-to-r from-brand to-brand-strong"
                />
              </div>
            </div>

            <div className="rounded-xl border border-border bg-surface-elevated p-4">
              <p className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
                Builds / hour
              </p>
              <p className="mt-2 text-2xl font-semibold tracking-tight text-foreground">12,408</p>
              <p className="mt-1 flex items-center gap-1 font-mono text-[11px] text-success">
                ↑ 23% vs last week
              </p>
            </div>

            <div className="rounded-xl border border-border bg-surface-elevated p-4">
              <p className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
                Time saved
              </p>
              <p className="mt-2 text-2xl font-semibold tracking-tight text-foreground">487h</p>
              <p className="mt-1 font-mono text-[11px] text-muted-foreground">this week</p>
            </div>
          </div>

          {/* Chart column: a flex column so the bars fill whatever height the
              stats column gives the row, instead of a fixed box in a tall card. */}
          <div className="flex flex-col rounded-xl border border-border bg-surface-elevated p-4">
            <div className="mb-3 flex items-center justify-between">
              <p className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
                Build wall-time · 7d
              </p>
              <div className="flex items-center gap-3 font-mono text-[10px]">
                <span className="flex items-center gap-1.5 text-muted-foreground">
                  <span className="inline-block h-1.5 w-3 rounded-sm bg-foreground/15" />
                  fresh
                </span>
                <span className="flex items-center gap-1.5 text-muted-foreground">
                  <span className="inline-block h-1.5 w-3 rounded-sm bg-gradient-to-t from-brand to-brand-strong" />
                  cached
                </span>
              </div>
            </div>

            {/* Bar chart: one group per day label, two bars per group. */}
            <div
              role="img"
              aria-label="Build wall-time over seven days: cached builds take roughly a third of the time of fresh builds every day."
              className="flex min-h-[148px] flex-1 items-end gap-2"
            >
              {days.map((day, i) => (
                <div key={day.label} className="flex h-full flex-1 items-end justify-center gap-1">
                  <motion.div
                    initial={{ height: 0 }}
                    whileInView={{ height: `${day.fresh}%` }}
                    viewport={{ once: true }}
                    transition={{ duration: 0.8, delay: i * 0.07, ease: [0.16, 1, 0.3, 1] }}
                    className="w-full rounded-t-sm bg-foreground/15"
                  />
                  <motion.div
                    initial={{ height: 0 }}
                    whileInView={{ height: `${day.cached}%` }}
                    viewport={{ once: true }}
                    transition={{ duration: 0.8, delay: i * 0.07 + 0.05, ease: [0.16, 1, 0.3, 1] }}
                    className="w-full rounded-t-sm bg-gradient-to-t from-brand to-brand-strong"
                  />
                </div>
              ))}
            </div>

            {/* X-axis: one label under each group */}
            <div className="mt-2 flex gap-2 font-mono text-[9px] text-muted-foreground">
              {days.map((day) => (
                <span key={day.label} className="flex-1 text-center">
                  {day.label}
                </span>
              ))}
            </div>
          </div>
        </div>

        {/* Bottom activity row */}
        <div className="border-t border-border/70 bg-surface-elevated/50 px-5 py-3">
          <p className="mb-2 font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
            Live activity
          </p>
          <div className="space-y-1.5 font-mono text-[11px] text-muted-foreground">
            <motion.div
              initial={{ opacity: 0, x: -8 }}
              whileInView={{ opacity: 1, x: 0 }}
              viewport={{ once: true }}
              transition={{ delay: 0.7 }}
              className="flex items-center gap-2"
            >
              <span className="inline-block h-1.5 w-1.5 rounded-full bg-success" />
              <span>cache hit</span>
              <span className="text-muted-foreground">bazel build //src/...</span>
              <span className="ml-auto text-muted">4.2s</span>
            </motion.div>
            <motion.div
              initial={{ opacity: 0, x: -8 }}
              whileInView={{ opacity: 1, x: 0 }}
              viewport={{ once: true }}
              transition={{ delay: 0.85 }}
              className="flex items-center gap-2"
            >
              <span className="inline-block h-1.5 w-1.5 rounded-full bg-brand" />
              <span>remote exec</span>
              <span className="text-muted-foreground">cargo test --release</span>
              <span className="ml-auto text-muted">12.8s</span>
            </motion.div>
            <motion.div
              initial={{ opacity: 0, x: -8 }}
              whileInView={{ opacity: 1, x: 0 }}
              viewport={{ once: true }}
              transition={{ delay: 1.0 }}
              className="flex items-center gap-2"
            >
              <span className="inline-block h-1.5 w-1.5 rounded-full bg-success" />
              <span>cache hit</span>
              <span className="text-muted-foreground">buck2 build :app</span>
              <span className="ml-auto text-muted">0.9s</span>
            </motion.div>
          </div>
        </div>
      </div>
    </div>
  );
}
