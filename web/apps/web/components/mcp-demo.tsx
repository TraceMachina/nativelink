"use client";

import { cn } from "@nativelink/ui";
import { AnimatePresence, motion, useInView } from "motion/react";
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
      { text: "# NativeLink Configuration", tone: "comment" },
      { text: "build --remote_cache=grpcs://cas.nativelink.internal" },
      { text: "build --remote_header=x-nativelink-api-key=<redacted>", tone: "muted" },
      { text: "build --remote_executor=grpcs://scheduler.nativelink.internal" },
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
      {
        text: "categories: setup · migration · optimization · troubleshooting · api",
        tone: "comment",
      },
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
      {
        text: '  { "invocationId": "…", "command": "bazel test //…", "exitCode": … },',
        tone: "muted",
      },
      {
        text: '  { "invocationId": "…", "command": "bazel build //…", "exitCode": … },',
        tone: "muted",
      },
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

/**
 * "wait": the demo is not on screen yet, show nothing.
 * "type": first look at this tool, type the prompt out.
 * "instant": the visitor has seen this tool before, show it whole.
 */
type TypingMode = "wait" | "type" | "instant";

function useTypewriter(text: string, mode: TypingMode, speed = 28) {
  const [out, setOut] = React.useState(mode === "instant" ? text : "");
  React.useEffect(() => {
    if (mode === "instant") {
      setOut(text);
      return;
    }
    setOut("");
    if (mode === "wait") {
      return;
    }
    let i = 0;
    const id = setInterval(() => {
      i += 1;
      setOut(text.slice(0, i));
      if (i >= text.length) clearInterval(id);
    }, speed);
    return () => clearInterval(id);
  }, [text, mode, speed]);
  return out;
}

export function McpDemo() {
  const [active, setActive] = React.useState(0);
  // Tools the visitor has already looked at: they render whole, no retyping.
  const [seen, setSeen] = React.useState<ReadonlySet<string>>(() => new Set());
  const rootRef = React.useRef<HTMLDivElement>(null);
  const inView = useInView(rootRef, { once: true, margin: "-80px" });

  const tool = tools[active] ?? (tools[0] as Tool);
  const mode: TypingMode = seen.has(tool.id) ? "instant" : inView ? "type" : "wait";
  const typed = useTypewriter(tool.prompt, mode);
  const done = typed.length >= tool.prompt.length;
  const instant = mode === "instant";

  const select = (next: number) => {
    const index = (next + tools.length) % tools.length;
    setSeen((prev) => (prev.has(tool.id) ? prev : new Set(prev).add(tool.id)));
    setActive(index);
    document.getElementById(`mcp-tab-${tools[index]?.id}`)?.focus();
  };

  const onTabKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "ArrowRight") {
      event.preventDefault();
      select(active + 1);
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      select(active - 1);
    } else if (event.key === "Home") {
      event.preventDefault();
      select(0);
    } else if (event.key === "End") {
      event.preventDefault();
      select(tools.length - 1);
    }
  };

  return (
    <div
      ref={rootRef}
      className="overflow-hidden rounded-2xl border border-border bg-surface shadow-[0_30px_80px_-30px_rgb(0_0_0_/_0.3)]"
    >
      <div className="flex items-center gap-2 border-b border-border bg-surface-elevated/60 px-4 py-3">
        <span className="h-3 w-3 rounded-full bg-border-strong" />
        <span className="h-3 w-3 rounded-full bg-border-strong" />
        <span className="h-3 w-3 rounded-full bg-border-strong" />
        <span className="ml-3 font-mono text-[11px] text-muted">nativelink-mcp · 5 tools</span>
        <span className="ml-auto hidden font-mono text-[11px] text-muted-foreground sm:inline">
          Pick a tool to see what it returns
        </span>
      </div>

      <div
        role="tablist"
        aria-label="NativeLink MCP tools"
        onKeyDown={onTabKeyDown}
        className="flex flex-wrap gap-1.5 border-b border-border px-3 py-3"
      >
        {tools.map((t, i) => {
          const selected = i === active;
          return (
            <button
              key={t.id}
              id={`mcp-tab-${t.id}`}
              type="button"
              role="tab"
              aria-selected={selected}
              aria-controls={`mcp-panel-${t.id}`}
              tabIndex={selected ? 0 : -1}
              onClick={() => select(i)}
              className={cn(
                "inline-flex min-h-[36px] cursor-pointer items-center rounded-full border px-3.5 font-mono text-[12px] transition-colors",
                "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60",
                selected
                  ? "border-brand/40 bg-brand-soft text-brand"
                  : "border-border bg-surface text-muted-foreground hover:border-border-strong hover:bg-surface-elevated hover:text-foreground",
              )}
            >
              {t.tab}
            </button>
          );
        })}
      </div>

      <div
        role="tabpanel"
        id={`mcp-panel-${tool.id}`}
        aria-labelledby={`mcp-tab-${tool.id}`}
        className="bg-background p-5 font-mono text-[13px] leading-relaxed md:p-6"
      >
        <p className="mb-1 text-[11px] uppercase tracking-[0.14em] text-muted-foreground/70">
          {tool.name}
        </p>
        <div className="flex gap-2 text-foreground/90">
          <span className="select-none text-brand">›</span>
          <span>
            {typed}
            {!done && (
              <span className="ml-0.5 inline-block h-4 w-2 animate-pulse bg-brand align-middle" />
            )}
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
                  transition={instant ? { duration: 0 } : { delay: i * 0.05, duration: 0.25 }}
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
