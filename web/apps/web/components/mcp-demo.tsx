"use client";

import { AnimatePresence, motion } from "motion/react";
import * as React from "react";

type Line = { text: string; tone?: "muted" | "brand" | "default" | "comment" };

interface Tool {
  id: string;
  tab: string;
  name: string;
  prompt: string;
  output: Line[];
}

const tools: Tool[] = [
  {
    id: "config",
    tab: "Bazel config",
    name: "Generate optimal Bazel configuration",
    prompt: "use nativelink to generate a bazel config for my Rust project",
    output: [
      { text: "# Nativelink Cloud Configuration", tone: "comment" },
      { text: "build --remote_cache=grpcs://cas.<account>.build-faster.nativelink.net" },
      { text: "build --remote_header=x-nativelink-api-key=<redacted>", tone: "muted" },
      { text: "build --remote_executor=grpcs://scheduler.<account>.build-faster.nativelink.net" },
      { text: "build --remote_timeout=600" },
      { text: "build --jobs=200" },
      { text: "build --remote_download_outputs=minimal" },
      { text: "build --experimental_remote_cache_compression" },
      { text: "# Rust Specific Configuration", tone: "comment" },
      { text: "build --@rules_rust//rust/settings:pipelined_compilation=True" },
    ],
  },
  {
    id: "docs",
    tab: "Docs",
    name: "Fetch documentation and best practices",
    prompt: "use nativelink to fetch optimization best practices",
    output: [
      { text: "# Nativelink Performance Optimization", tone: "comment" },
      { text: "## Cache Optimization", tone: "brand" },
      { text: "1. Increase cache hit rate" },
      { text: "   - Use --experimental_strict_action_env", tone: "muted" },
      { text: "   - Set --incompatible_strict_action_env=true", tone: "muted" },
      { text: "## Cost Optimization", tone: "brand" },
      { text: "   - Use --remote_download_minimal", tone: "muted" },
      { text: "   - Enable --experimental_remote_cache_compression", tone: "muted" },
      { text: "categories: setup · migration · optimization · troubleshooting · api", tone: "comment" },
    ],
  },
  {
    id: "performance",
    tab: "Performance",
    name: "Analyze build performance",
    prompt: "use nativelink to analyze my build performance (balanced)",
    output: [
      { text: "# Analyzing cacheHitRate, totalTime, remoteExecutionTime …", tone: "comment" },
      { text: "## Optimization Recommendations", tone: "brand" },
      { text: "### Balanced Optimizations", tone: "brand" },
      { text: "build --jobs=100" },
      { text: "build --remote_download_outputs=minimal" },
      { text: "build --experimental_remote_cache_compression" },
      { text: "build --remote_timeout=60" },
      { text: "# Flags low cache-hit-rate and large network transfers", tone: "comment" },
      { text: "# Strategies: speed · cost · balanced", tone: "comment" },
    ],
  },
  {
    id: "watch",
    tab: "Watch & build",
    name: "Watch files, build and test on change",
    prompt: "use nativelink to watch and rebuild on file changes",
    output: [
      { text: "# Automatic Build with iBazel (Recommended)", tone: "comment" },
      { text: "## Installation", tone: "brand" },
      { text: "npm install -g @bazel/ibazel" },
      { text: "## Usage", tone: "brand" },
      { text: "ibazel test //...", tone: "brand" },
      { text: "# Modes: build · test · both — with debounce + ignore paths", tone: "comment" },
      { text: "# Uses your .bazelrc for remote cache + execution", tone: "comment" },
    ],
  },
  {
    id: "builds",
    tab: "Recent builds",
    name: "Get information about recent builds",
    prompt: "use nativelink to show my recent builds",
    output: [
      { text: "# Recent builds (last 10) for account: <redacted>", tone: "comment" },
      { text: "[" },
      { text: '  { "invocationId": "…", "command": "bazel test //…", "exitCode": … },', tone: "muted" },
      { text: '  { "invocationId": "…", "command": "bazel build //…", "exitCode": … },', tone: "muted" },
      { text: "  …" },
      { text: "]" },
      { text: "# Streamed from the Build Event Protocol", tone: "comment" },
    ],
  },
];

const toneClass: Record<NonNullable<Line["tone"]>, string> = {
  default: "text-foreground/90",
  muted: "text-muted",
  brand: "text-brand",
  comment: "text-muted-foreground/70",
};

function useTypewriter(text: string, active: boolean, speed = 28) {
  const [out, setOut] = React.useState("");
  React.useEffect(() => {
    if (!active) return;
    setOut("");
    let i = 0;
    const id = setInterval(() => {
      i += 1;
      setOut(text.slice(0, i));
      if (i >= text.length) clearInterval(id);
    }, speed);
    return () => clearInterval(id);
  }, [text, active, speed]);
  return out;
}

export function McpDemo() {
  const [active, setActive] = React.useState(0);
  const tool: Tool = tools[active] ?? tools[0]!;
  const typed = useTypewriter(tool.prompt, true);
  const done = typed.length >= tool.prompt.length;

  return (
    <div className="overflow-hidden rounded-2xl border border-border bg-surface shadow-[0_30px_80px_-30px_rgb(0_0_0_/_0.3)]">
      <div className="flex items-center gap-2 border-b border-border bg-surface-elevated/60 px-4 py-3">
        <span className="h-3 w-3 rounded-full bg-border-strong" />
        <span className="h-3 w-3 rounded-full bg-border-strong" />
        <span className="h-3 w-3 rounded-full bg-border-strong" />
        <span className="ml-3 font-mono text-[11px] text-muted">nativelink-mcp · 5 tools</span>
      </div>

      <div className="flex flex-wrap gap-1 border-b border-border px-3 pt-3">
        {tools.map((t, i) => (
          <button
            key={t.id}
            type="button"
            onClick={() => setActive(i)}
            className={`rounded-t-md px-3 py-2 font-mono text-[12px] transition-colors ${
              i === active
                ? "bg-background text-brand"
                : "text-muted-foreground hover:text-foreground"
            }`}
          >
            {t.tab}
          </button>
        ))}
      </div>

      <div className="bg-background p-5 font-mono text-[13px] leading-relaxed md:p-6">
        <p className="mb-1 text-[11px] uppercase tracking-[0.14em] text-muted-foreground/70">
          {tool.name}
        </p>
        <div className="flex gap-2 text-foreground/90">
          <span className="select-none text-brand">›</span>
          <span>
            {typed}
            {!done && <span className="ml-0.5 inline-block h-4 w-2 animate-pulse bg-brand align-middle" />}
          </span>
        </div>

        <AnimatePresence mode="wait">
          {done && (
            <motion.div
              key={tool.id}
              initial="hidden"
              animate="visible"
              className="mt-3 space-y-0.5"
            >
              {tool.output.map((line, i) => (
                <motion.div
                  key={`${tool.id}-${i}`}
                  variants={{
                    hidden: { opacity: 0, y: 4 },
                    visible: { opacity: 1, y: 0 },
                  }}
                  initial="hidden"
                  animate="visible"
                  transition={{ delay: i * 0.05, duration: 0.25 }}
                  className={`whitespace-pre-wrap break-words ${toneClass[line.tone ?? "default"]}`}
                >
                  {line.text}
                </motion.div>
              ))}
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}
