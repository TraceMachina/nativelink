import * as React from "react";
import { cn } from "../lib/cn";

interface StatProps extends React.HTMLAttributes<HTMLDivElement> {
  value: React.ReactNode;
  label: React.ReactNode;
  accent?: boolean;
}

export function Stat({ value, label, accent = false, className, ...props }: StatProps) {
  return (
    <div className={cn("flex flex-col gap-2", className)} {...props}>
      <div
        className={cn(
          "font-mono text-3xl font-semibold leading-none tracking-tight md:text-4xl",
          accent ? "text-brand" : "text-foreground",
        )}
      >
        {value}
      </div>
      <div className="text-sm leading-relaxed text-muted">{label}</div>
    </div>
  );
}

export function StatGrid({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "grid grid-cols-2 gap-x-8 gap-y-10 md:grid-cols-4",
        className,
      )}
      {...props}
    />
  );
}
