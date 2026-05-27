import { cn } from "@nativelink/ui";
import type * as React from "react";

interface CalloutProps {
  type?: "info" | "warn" | "success" | "error";
  title?: string;
  children: React.ReactNode;
  className?: string;
}

const tones: Record<NonNullable<CalloutProps["type"]>, string> = {
  info: "border-brand/40 bg-brand-soft/40 text-foreground",
  warn: "border-amber-500/40 bg-amber-50/60 text-foreground dark:bg-amber-500/10",
  success: "border-success/40 bg-success-soft/60 text-foreground",
  error: "border-red-500/40 bg-red-50/60 text-foreground dark:bg-red-500/10",
};

const iconPaths: Record<NonNullable<CalloutProps["type"]>, React.ReactNode> = {
  info: <path d="M12 16v-4M12 8h.01M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10Z" />,
  warn: <path d="M12 9v4m0 4h.01M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0Z" />,
  success: <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14M22 4 12 14.01l-3-3" />,
  error: <path d="M15 9l-6 6m0-6 6 6M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />,
};

export function Callout({ type = "info", title, children, className }: CalloutProps) {
  return (
    <div
      className={cn(
        "my-6 flex gap-3 rounded-xl border px-5 py-4",
        tones[type],
        className,
      )}
      role={type === "error" || type === "warn" ? "alert" : "note"}
    >
      <svg
        width="20"
        height="20"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.75"
        strokeLinecap="round"
        strokeLinejoin="round"
        className={cn(
          "mt-0.5 shrink-0",
          type === "info" && "text-brand",
          type === "warn" && "text-amber-600 dark:text-amber-400",
          type === "success" && "text-success",
          type === "error" && "text-red-600 dark:text-red-400",
        )}
        aria-hidden="true"
      >
        {iconPaths[type]}
      </svg>
      <div className="flex-1 [&>p]:my-0 [&>p+p]:mt-2">
        {title ? (
          <p className="mb-1 font-mono text-xs font-semibold uppercase tracking-[0.12em]">
            {title}
          </p>
        ) : null}
        {children}
      </div>
    </div>
  );
}
