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

/** One invocation, summarised the way a build UI would, so the panel reads as a build tool's view. */
const invocation = [
  { label: "actions", value: "8,912" },
  { label: "cache hits", value: "8,217" },
  { label: "remote", value: "695" },
  { label: "wall time", value: "2m 04s" },
];

const tabs = ["Targets", "Timing", "Cache", "Executions"];

/** Recent activity, including the misses: a dashboard that only shows green is not a dashboard. */
const activity = [
  {
    kind: "cache hit",
    dot: "bg-success",
    command: "bazel build //src/...",
    detail: "",
    time: "4.2s",
  },
  {
    kind: "remote exec",
    dot: "bg-brand",
    command: "cargo test --release",
    detail: "48 workers",
    time: "12.8s",
  },
  {
    kind: "cache miss",
    dot: "bg-foreground/35",
    command: "//compiler:parser_test",
    detail: "recompiled",
    time: "31.4s",
  },
  { kind: "cache hit", dot: "bg-success", command: "buck2 build :app", detail: "", time: "0.9s" },
  {
    kind: "remote exec",
    dot: "bg-brand",
    command: "siso ninja -C out/Default",
    detail: "212 workers",
    time: "3m 10s",
  },
  {
    kind: "cache hit",
    dot: "bg-success",
    command: "recc clang++ -c main.cc",
    detail: "",
    time: "0.3s",
  },
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
            NativeLink · build events
          </div>
          <div className="ml-auto flex items-center gap-1">
            <span className="rounded-md bg-foreground/5 px-2 py-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
              sample data
            </span>
          </div>
        </div>

        {/* Invocation summary + tab strip: the chrome of a build UI, drawn not shot */}
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border/70 px-5 py-3 font-mono text-[11px]">
          <span className="text-foreground">{"bazel test //..."}</span>
          {invocation.map((item) => (
            <span key={item.label} className="flex items-center gap-1 text-muted-foreground">
              <span className="font-semibold text-foreground">{item.value}</span>
              {item.label}
            </span>
          ))}
          <span className="ml-auto flex items-center gap-1.5 text-success">
            <span className="inline-block h-1.5 w-1.5 rounded-full bg-success" />
            succeeded
          </span>
        </div>
        <div className="flex gap-1 border-b border-border/70 px-4 pt-2" aria-hidden="true">
          {tabs.map((tab) => (
            <span
              key={tab}
              className={cn(
                "rounded-t-md px-3 py-1.5 font-mono text-[10px] uppercase tracking-wider",
                tab === "Cache" ? "border-b-2 border-brand text-brand" : "text-muted-foreground",
              )}
            >
              {tab}
            </span>
          ))}
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
                Build wall-time · minutes · 7d
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
              aria-label="Sample chart of build wall-time in minutes over seven days: cached builds take roughly a third of the time of fresh builds every day."
              className="flex min-h-[148px] flex-1 gap-2"
            >
              <div className="flex h-full flex-col justify-between pb-0.5 font-mono text-[9px] text-muted">
                <span>40</span>
                <span>20</span>
                <span>0</span>
              </div>
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
            <div className="mt-2 flex gap-2 pl-5 font-mono text-[9px] text-muted-foreground">
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
            {activity.map((row, i) => (
              <motion.div
                key={`${row.command}-${i}`}
                initial={{ opacity: 0, x: -8 }}
                whileInView={{ opacity: 1, x: 0 }}
                viewport={{ once: true }}
                transition={{ delay: 0.7 + i * 0.12 }}
                className="flex items-center gap-2"
              >
                <span className={cn("inline-block h-1.5 w-1.5 shrink-0 rounded-full", row.dot)} />
                <span className="w-[5.5rem] shrink-0">{row.kind}</span>
                <span className="truncate text-foreground/80">{row.command}</span>
                {row.detail ? (
                  <span className="hidden text-muted sm:inline">· {row.detail}</span>
                ) : null}
                <span className="ml-auto shrink-0 text-muted">{row.time}</span>
              </motion.div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
