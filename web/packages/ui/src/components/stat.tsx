import type * as React from "react";
import { cn } from "../lib/cn";

interface StatProps extends React.HTMLAttributes<HTMLDivElement> {
  /** The number, e.g. "4-15×" or "0 rewrites". Keep the noun in here when a bare digit would mislead. */
  value: React.ReactNode;
  /** What the number is, in bold. */
  label: React.ReactNode;
  /** One supporting sentence, muted. */
  body?: React.ReactNode;
  /** Brand-coloured value. Rows should be all-accent or all-plain, never mixed. */
  accent?: boolean;
}

/**
 * The one way a proof number is set on the marketing site: mono value, bold
 * label, optional muted body. Home and Product both render their numbers
 * through this, so the same claim cannot drift in weight or wording.
 */
export function Stat({ value, label, body, accent = true, className, ...props }: StatProps) {
  return (
    <div className={cn("flex flex-col gap-2", className)} {...props}>
      <div
        className={cn(
          "whitespace-nowrap font-mono text-3xl font-semibold leading-none tracking-tight",
          accent ? "text-brand" : "text-foreground",
        )}
      >
        {value}
      </div>
      <div className="text-base font-semibold text-foreground">{label}</div>
      {body ? <div className="text-sm leading-relaxed text-muted">{body}</div> : null}
    </div>
  );
}

interface StatGridProps extends React.HTMLAttributes<HTMLDivElement> {
  columns?: 2 | 3 | 4;
}

const columnClass: Record<NonNullable<StatGridProps["columns"]>, string> = {
  2: "md:grid-cols-2",
  3: "md:grid-cols-3",
  // Four across only from xl: at lg a 220px column cannot hold a two-word value on one line.
  4: "md:grid-cols-2 xl:grid-cols-4",
};

export function StatGrid({ columns = 4, className, ...props }: StatGridProps) {
  return (
    <div
      className={cn("grid grid-cols-1 gap-x-8 gap-y-10", columnClass[columns], className)}
      {...props}
    />
  );
}
