import type { ReactNode } from "react";
import { cn } from "../lib/cn";

/**
 * The "Ask AI" block: one-click hand-off of this site to a hosted assistant,
 * plus the three machine-readable corpus files every page of the site serves.
 *
 * It renders on the server (no hooks) and ships no JavaScript. Everything
 * carries a `data-ask-ai` attribute so a crawler can find the block without
 * parsing prose.
 */

export interface AskAiProps {
  /** The question the assistant buttons open with. */
  prompt?: string;
  /** Absolute origin whose llms files to link, e.g. "https://nativelink.com".
   *  Defaults to site-relative links, which is right on either site. */
  siteUrl?: string;
  heading?: string;
  /** `center` for a footer or a hero card; `start` inside left-aligned prose. */
  align?: "start" | "center";
  /** `sm` is for the bottom of a docs page; `md` for footers and sections. */
  size?: "sm" | "md";
  className?: string;
}

/**
 * The question the footer and the agents page open with. It points the
 * assistant at the corpus, asks for the product's features rather than a
 * comparison, then steers the conversation from the reader's own use case
 * towards a configuration and deployment for it. A deployment shared by more
 * than one developer exercises the Business Source Licensed modules (metrics
 * cannot be switched off), so the prompt routes that case to the pricing
 * inbox rather than letting the assistant guess.
 */
export function askAiPrompt(
  siteUrl = "https://nativelink.com",
  docsUrl = "https://docs.nativelink.com",
): string {
  return [
    `Read ${siteUrl}/llms.txt and ${docsUrl}/llms.txt first.`,
    "Then explain what NativeLink is and what it can do: the remote build cache, remote",
    "execution, the store backends it supports, and the build tools it connects to.",
    "Ask me about my use case (which build tool, how many developers, where it will run, and",
    "whether I need caching only or execution too), then recommend a configuration and",
    "deployment for it, citing the matching documentation pages.",
    "If more than one developer will share the deployment, tell me that its metrics and",
    "persistent-worker modules are Business Source Licensed and that I should email",
    "contact@nativelink.com for Enterprise pricing on them.",
  ].join(" ");
}

/** Where a query lands for each assistant. Every one of these accepts a `q`
 *  parameter that pre-fills the first message. */
const assistants: { name: string; url: (q: string) => string; icon: ReactNode }[] = [
  {
    name: "ChatGPT",
    url: (q) => `https://chatgpt.com/?q=${q}`,
    icon: (
      <>
        <path d="M12 3l7.8 4.5v9L12 21l-7.8-4.5v-9L12 3z" />
        <path d="M12 8l3.46 2v4L12 16l-3.46-2v-4L12 8z" />
      </>
    ),
  },
  {
    name: "Claude",
    url: (q) => `https://claude.ai/new?q=${q}`,
    icon: (
      <path
        d="M12 3v18M3 12h18M5.64 5.64l12.72 12.72M18.36 5.64L5.64 18.36"
        strokeLinecap="round"
      />
    ),
  },
  {
    name: "Perplexity",
    url: (q) => `https://www.perplexity.ai/search/?q=${q}`,
    icon: (
      <>
        <path d="M12 3l9 9-9 9-9-9 9-9z" />
        <path d="M12 3v18M3 12h18" strokeLinecap="round" />
      </>
    ),
  },
  {
    name: "Google AI",
    url: (q) => `https://www.google.com/search?udm=50&q=${q}`,
    icon: (
      <path
        d="M12 2c.6 5.5 4.5 9.4 10 10-5.5.6-9.4 4.5-10 10-.6-5.5-4.5-9.4-10-10 5.5-.6 9.4-4.5 10-10z"
        fill="currentColor"
        stroke="none"
      />
    ),
  },
  {
    name: "Copilot",
    url: (q) => `https://copilot.microsoft.com/?q=${q}`,
    icon: (
      <>
        <rect x="3" y="8" width="9.5" height="8" rx="4" />
        <rect x="11.5" y="8" width="9.5" height="8" rx="4" />
      </>
    ),
  },
];

const files: { name: string; path: string; description: string }[] = [
  {
    name: "llms.txt",
    path: "/llms.txt",
    description: "the link index: every page with a one-line description, in reading order",
  },
  {
    name: "llms-small.txt",
    path: "/llms-small.txt",
    description:
      "the abridged corpus: every page's headings and opening sentences, without code samples",
  },
  {
    name: "llms-full.txt",
    path: "/llms-full.txt",
    description: "the whole corpus, every page body verbatim, in one fetch",
  },
];

const robotIcon = (
  <>
    <rect x="4" y="8" width="16" height="11" rx="3" />
    <path d="M12 8V5" />
    <circle cx="12" cy="4" r="1" />
    <path d="M9 13h.01M15 13h.01" strokeLinecap="round" strokeWidth="2.5" />
    <path d="M9.5 16h5" strokeLinecap="round" />
  </>
);

function Icon({ children, size }: { children: ReactNode; size: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinejoin="round"
      aria-hidden="true"
      className="shrink-0"
    >
      {children}
    </svg>
  );
}

export function AskAi({
  prompt = askAiPrompt(),
  siteUrl = "",
  heading = "Ask AI about this page",
  size = "md",
  align = "start",
  className,
}: AskAiProps) {
  const q = encodeURIComponent(prompt);
  let base = siteUrl;
  while (base.endsWith("/")) base = base.slice(0, -1);
  const chip = cn(
    "inline-flex cursor-pointer items-center gap-2 rounded-xl border border-border bg-surface text-foreground",
    "transition-colors hover:border-foreground hover:bg-foreground hover:text-background",
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60 focus-visible:ring-offset-2 focus-visible:ring-offset-background",
    size === "sm" ? "h-9 px-3 text-[13px]" : "h-11 px-4 text-[15px]",
  );

  return (
    <div
      data-ask-ai
      className={cn(
        "flex flex-col gap-4",
        align === "center" && "items-center text-center",
        className,
      )}
    >
      <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-muted">{heading}</p>

      <ul
        className={cn("flex flex-wrap gap-2", align === "center" && "justify-center")}
        aria-label="Open this site in an AI assistant"
      >
        {assistants.map((a) => (
          <li key={a.name}>
            <a
              href={a.url(q)}
              target="_blank"
              rel="noreferrer"
              data-ask-ai-assistant={a.name}
              aria-label={`Ask ${a.name} about NativeLink`}
              className={cn(chip, "font-medium")}
            >
              <Icon size={size === "sm" ? 15 : 17}>{a.icon}</Icon>
              {a.name}
            </a>
          </li>
        ))}
      </ul>

      <ul
        className={cn("flex flex-wrap gap-2", align === "center" && "justify-center")}
        aria-label="Machine-readable copies of this site"
      >
        {files.map((f) => (
          <li key={f.name}>
            <a
              href={`${base}${f.path}`}
              data-ask-ai-file={f.name}
              title={f.description}
              className={cn(chip, "font-mono")}
            >
              <Icon size={size === "sm" ? 15 : 17}>{robotIcon}</Icon>
              {f.name}
            </a>
          </li>
        ))}
      </ul>
    </div>
  );
}
