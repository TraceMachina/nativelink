"use client";

import * as motion from "motion/react-client";
import { cn } from "../lib/cn";

interface HeroVisualProps {
  className?: string;
}

const bars = [
  { delay: 0.0, height: 28 },
  { delay: 0.05, height: 52 },
  { delay: 0.1, height: 38 },
  { delay: 0.15, height: 72 },
  { delay: 0.2, height: 44 },
  { delay: 0.25, height: 88 },
  { delay: 0.3, height: 60 },
  { delay: 0.35, height: 96 },
  { delay: 0.4, height: 76 },
  { delay: 0.45, height: 110 },
  { delay: 0.5, height: 92 },
  { delay: 0.55, height: 124 },
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
              <p className="mt-2 text-2xl font-semibold tracking-tight text-foreground">
                94.7%
              </p>
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
              <p className="mt-2 text-2xl font-semibold tracking-tight text-foreground">
                12,408
              </p>
              <p className="mt-1 flex items-center gap-1 font-mono text-[11px] text-success">
                ↑ 23% vs last week
              </p>
            </div>

            <div className="rounded-xl border border-border bg-surface-elevated p-4">
              <p className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
                Time saved
              </p>
              <p className="mt-2 text-2xl font-semibold tracking-tight text-foreground">
                487h
              </p>
              <p className="mt-1 font-mono text-[11px] text-muted-foreground">
                this week
              </p>
            </div>
          </div>

          {/* Chart column */}
          <div className="rounded-xl border border-border bg-surface-elevated p-4">
            <div className="mb-3 flex items-center justify-between">
              <p className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
                Build wall-time · 7d
              </p>
              <div className="flex items-center gap-3 font-mono text-[10px]">
                <span className="flex items-center gap-1.5 text-muted-foreground">
                  <span className="inline-block h-1.5 w-3 rounded-sm bg-brand" />
                  cached
                </span>
                <span className="flex items-center gap-1.5 text-muted-foreground">
                  <span className="inline-block h-1.5 w-3 rounded-sm bg-foreground/15" />
                  fresh
                </span>
              </div>
            </div>

            {/* Bar chart */}
            <div className="flex h-[148px] items-end justify-between gap-1.5 pl-2">
              {bars.map((bar, i) => (
                <div key={i} className="flex h-full flex-1 items-end gap-0">
                  <motion.div
                    initial={{ height: 0 }}
                    whileInView={{ height: `${bar.height * 0.7}%` }}
                    viewport={{ once: true }}
                    transition={{ duration: 0.8, delay: bar.delay, ease: [0.16, 1, 0.3, 1] }}
                    className="w-full rounded-t-sm bg-gradient-to-t from-brand to-brand-strong"
                  />
                </div>
              ))}
            </div>

            {/* X-axis */}
            <div className="mt-2 flex justify-between font-mono text-[9px] text-muted-foreground">
              {["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].map((d) => (
                <span key={d}>{d}</span>
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
              <span className="text-foreground/40">bazel build //src/...</span>
              <span className="ml-auto text-foreground/40">4.2s</span>
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
              <span className="text-foreground/40">cargo test --release</span>
              <span className="ml-auto text-foreground/40">12.8s</span>
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
              <span className="text-foreground/40">buck2 build :app</span>
              <span className="ml-auto text-foreground/40">0.9s</span>
            </motion.div>
          </div>
        </div>
      </div>
    </div>
  );
}
