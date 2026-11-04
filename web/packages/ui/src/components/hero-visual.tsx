"use client";

import * as motion from "motion/react-client";
import { cn } from "../lib/cn";

interface HeroVisualProps {
  className?: string;
}

/**
 * One Bazel invocation, drawn the way a build UI would show it. Every number
 * on the panel is about this build and agrees with the others: 8,217 of
 * 8,912 actions were cache hits (92.2%), the remaining 695 ran remotely on
 * 48 workers, 31m 06s of action time finished in 2m 04s of wall time.
 * Sample data, to be replaced by a real screenshot when one can be published.
 */
const invocation = {
  command: "bazel test //...",
  id: "a3f9c2",
  status: "succeeded",
  wallTime: "2m 04s",
  chips: ["Bazel 8.4.1", "k8", "fastbuild", "4,210 targets", "8,912 actions"],
  actions: 8912,
  cacheHits: 8217,
  remote: 695,
  workers: 48,
  actionTime: "31m 06s",
  parallelism: "15×",
};

const hitRate = ((invocation.cacheHits / invocation.actions) * 100).toFixed(1);

const tabs = ["Targets", "Timing", "Cache", "Executions"];

/** The slowest remote actions of this build, split into time queued and time executing. */
const slowest = [
  { label: "//src/compiler:parser_test", queued: 2.1, executing: 29.3 },
  { label: "//src/runtime:scheduler_test", queued: 1.4, executing: 24.0 },
  { label: "//src/compiler:codegen", queued: 3.2, executing: 18.6 },
  { label: "//src/proto:build_event_cc_proto", queued: 0.8, executing: 12.0 },
  { label: "//src/store:redis_store_test", queued: 2.6, executing: 9.9 },
  { label: "//src/worker:sandbox_test", queued: 0.5, executing: 7.3 },
  { label: "//src/util:digest_test", queued: 0.3, executing: 4.1 },
];
const axisMax = 32;
const axisTicks = ["0", "8", "16", "24", "32s"];

/** The action log of this build, newest first: cache hits in milliseconds, remote work with its worker. */
const actions = [
  {
    kind: "cache hit",
    dot: "bg-success",
    label: "//third_party/zstd:zstd",
    detail: "",
    time: "0.02s",
  },
  {
    kind: "remote exec",
    dot: "bg-brand",
    label: "//src/compiler:parser_test",
    detail: "worker us-east-1b/17",
    time: "31.4s",
  },
  { kind: "cache hit", dot: "bg-success", label: "//src/util:digest", detail: "", time: "0.01s" },
  {
    kind: "remote exec",
    dot: "bg-brand",
    label: "//src/proto:build_event_cc_proto",
    detail: "worker us-east-1b/04",
    time: "12.8s",
  },
  {
    kind: "cache hit",
    dot: "bg-success",
    label: "//src/compiler:lexer",
    detail: "",
    time: "0.03s",
  },
  {
    kind: "remote exec",
    dot: "bg-brand",
    label: "//src/store:redis_store_test",
    detail: "worker us-east-1b/22",
    time: "12.5s",
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
            NativeLink · invocation {invocation.id}
          </div>
          <div className="ml-auto flex items-center gap-1">
            <span className="rounded-md bg-foreground/5 px-2 py-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
              sample data
            </span>
          </div>
        </div>

        {/* Invocation header: what ran, how it went, and the facts a build page carries */}
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border/70 px-5 py-3 font-mono text-[11px]">
          <span className="font-semibold text-foreground">{invocation.command}</span>
          <span className="flex items-center gap-1.5 text-success">
            <span className="inline-block h-1.5 w-1.5 rounded-full bg-success" />
            {invocation.status}
          </span>
          <span className="text-foreground">{invocation.wallTime}</span>
          {invocation.chips.map((chip) => (
            <span
              key={chip}
              className="rounded-md border border-border/70 bg-background/60 px-1.5 py-0.5 text-muted-foreground"
            >
              {chip}
            </span>
          ))}
        </div>
        <div className="flex gap-1 border-b border-border/70 px-4 pt-2" aria-hidden="true">
          {tabs.map((tab) => (
            <span
              key={tab}
              className={cn(
                "rounded-t-md px-3 py-1.5 font-mono text-[10px] uppercase tracking-wider",
                tab === "Timing" ? "border-b-2 border-brand text-brand" : "text-muted-foreground",
              )}
            >
              {tab}
            </span>
          ))}
        </div>

        {/* Body */}
        <div className="grid grid-cols-[1fr_1.4fr] gap-4 p-5">
          {/* Facts about this build */}
          <div className="flex flex-col gap-3">
            <div className="rounded-xl border border-border bg-surface-elevated p-4">
              <p className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
                Cache hit rate
              </p>
              <p className="mt-2 text-2xl font-semibold tracking-tight text-foreground">
                {hitRate}%
              </p>
              <div className="mt-3 h-1.5 w-full overflow-hidden rounded-full bg-foreground/[0.06]">
                <motion.div
                  initial={{ width: 0 }}
                  whileInView={{ width: `${hitRate}%` }}
                  viewport={{ once: true }}
                  transition={{ duration: 1.2, delay: 0.3, ease: [0.16, 1, 0.3, 1] }}
                  className="h-full rounded-full bg-gradient-to-r from-brand to-brand-strong"
                />
              </div>
              <p className="mt-2 font-mono text-[11px] text-muted-foreground">
                {invocation.cacheHits.toLocaleString("en-US")} of{" "}
                {invocation.actions.toLocaleString("en-US")} actions
              </p>
            </div>

            <div className="rounded-xl border border-border bg-surface-elevated p-4">
              <p className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
                Remote actions
              </p>
              <p className="mt-2 text-2xl font-semibold tracking-tight text-foreground">
                {invocation.remote}
              </p>
              <p className="mt-1 font-mono text-[11px] text-muted-foreground">
                on {invocation.workers} workers
              </p>
            </div>

            <div className="rounded-xl border border-border bg-surface-elevated p-4">
              <p className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
                Work compressed
              </p>
              <p className="mt-2 text-2xl font-semibold tracking-tight text-foreground">
                {invocation.actionTime}
              </p>
              <p className="mt-1 font-mono text-[11px] text-muted-foreground">
                of action time in {invocation.wallTime} ·{" "}
                <span className="text-success">{invocation.parallelism} parallel</span>
              </p>
            </div>
          </div>

          {/* Timing: the slowest actions of this build, queued vs executing */}
          <div className="flex flex-col rounded-xl border border-border bg-surface-elevated p-4">
            <div className="mb-3 flex items-center justify-between">
              <p className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
                Slowest actions · seconds
              </p>
              <div className="flex items-center gap-3 font-mono text-[10px]">
                <span className="flex items-center gap-1.5 text-muted-foreground">
                  <span className="inline-block h-1.5 w-3 rounded-sm bg-foreground/15" />
                  queued
                </span>
                <span className="flex items-center gap-1.5 text-muted-foreground">
                  <span className="inline-block h-1.5 w-3 rounded-sm bg-gradient-to-r from-brand to-brand-strong" />
                  executing
                </span>
              </div>
            </div>

            <div
              role="img"
              aria-label={`Sample chart of the seven slowest actions in this build, each split into time queued and time executing. The slowest, ${slowest[0]?.label}, took ${slowest[0] ? (slowest[0].queued + slowest[0].executing).toFixed(1) : ""} seconds.`}
              className="flex min-h-[148px] flex-1 flex-col justify-between gap-2"
            >
              {slowest.map((action, i) => (
                <div key={action.label} className="flex items-center gap-3">
                  <span className="w-[42%] shrink-0 truncate font-mono text-[10px] text-muted-foreground">
                    {action.label}
                  </span>
                  <div className="flex h-2.5 flex-1 overflow-hidden rounded-sm bg-foreground/[0.04]">
                    <motion.div
                      initial={{ width: 0 }}
                      whileInView={{ width: `${(action.queued / axisMax) * 100}%` }}
                      viewport={{ once: true }}
                      transition={{ duration: 0.6, delay: i * 0.06, ease: [0.16, 1, 0.3, 1] }}
                      className="h-full bg-foreground/15"
                    />
                    <motion.div
                      initial={{ width: 0 }}
                      whileInView={{ width: `${(action.executing / axisMax) * 100}%` }}
                      viewport={{ once: true }}
                      transition={{ duration: 0.8, delay: i * 0.06 + 0.1, ease: [0.16, 1, 0.3, 1] }}
                      className="h-full bg-gradient-to-r from-brand to-brand-strong"
                    />
                  </div>
                </div>
              ))}
            </div>

            {/* Time axis under the bars */}
            <div className="mt-2 flex pl-[calc(42%+0.75rem)] font-mono text-[9px] text-muted-foreground">
              <div className="flex flex-1 justify-between">
                {axisTicks.map((tick) => (
                  <span key={tick}>{tick}</span>
                ))}
              </div>
            </div>
          </div>
        </div>

        {/* Action log */}
        <div className="border-t border-border/70 bg-surface-elevated/50 px-5 py-3">
          <p className="mb-2 font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
            Actions · newest first
          </p>
          <div className="space-y-1.5 font-mono text-[11px] text-muted-foreground">
            {actions.map((row, i) => (
              <motion.div
                key={row.label}
                initial={{ opacity: 0, x: -8 }}
                whileInView={{ opacity: 1, x: 0 }}
                viewport={{ once: true }}
                transition={{ delay: 0.7 + i * 0.12 }}
                className="flex items-center gap-2"
              >
                <span className={cn("inline-block h-1.5 w-1.5 shrink-0 rounded-full", row.dot)} />
                <span className="w-[5.5rem] shrink-0">{row.kind}</span>
                <span className="truncate text-foreground/80">{row.label}</span>
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
